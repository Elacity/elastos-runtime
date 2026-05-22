//! `elastos vm-debug …` — developer-only entry points for
//! driving the Apple Silicon Vz backend directly, without going
//! through the supervisor's normal capsule lifecycle.
//!
//! Phase 2 Day 4 of `docs/vz-backend/PLAN.md`. The subcommand
//! exists so a contributor on a Mac can verify their codesigned
//! `elastos` binary actually boots a guest VM end-to-end against
//! their own kernel + rootfs, before Phase 3 reroutes the
//! supervisor through this same wiring.
//!
//! Non-macOS hosts get a typed error pointing at `docs/MAC.md`;
//! everything stays compiling on Linux because the macOS-only
//! implementation lives behind a single `cfg` block.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::Subcommand;

#[cfg(not(target_os = "macos"))]
use anyhow::anyhow;

/// Default boot-args for `vm-debug boot`. Tuned for "any
/// reasonable Linux rootfs" — sets the kernel console to the
/// Vz-mandated `hvc0`, gates `reboot` to `k` (so the guest
/// hangs deterministically on crash), and roots the kernel at
/// the first virtio block device.
///
/// `VmConfig::from_manifest` does not rewrite this string when
/// it already says `console=hvc0`, so what you set is what the
/// guest sees.
pub(crate) const VM_DEBUG_DEFAULT_BOOT_ARGS: &str =
    "console=hvc0 reboot=k panic=1 root=/dev/vda rw";

/// Subcommands under `elastos vm-debug`.
#[derive(Subcommand, Debug)]
pub(crate) enum VmDebugCommand {
    /// Boot a Linux guest end-to-end against a user-supplied
    /// kernel + rootfs and stream the guest console to stdout.
    /// macOS / Apple Silicon only — see `docs/MAC.md`.
    Boot(BootArgs),
}

#[derive(clap::Args, Debug)]
pub(crate) struct BootArgs {
    /// Path to the guest rootfs disk image. Will be exposed as
    /// `/dev/vda` (first virtio block device).
    #[arg(long)]
    pub rootfs: PathBuf,

    /// Path to the guest kernel Image (NOT bzImage — Vz needs
    /// a raw Linux kernel image, e.g. arch/arm64/boot/Image).
    #[arg(long)]
    pub kernel: PathBuf,

    /// Guest memory in MiB.
    #[arg(long, default_value_t = 256)]
    pub memory_mb: u32,

    /// Guest vCPU count.
    #[arg(long, default_value_t = 1)]
    pub vcpus: u8,

    /// Override the default kernel command line.
    #[arg(long)]
    pub boot_args: Option<String>,
}

/// Entry point reached from `main.rs`. Dispatches to the
/// macOS-only implementation or, on Linux, returns a typed
/// error.
pub(crate) async fn run(cmd: VmDebugCommand) -> Result<()> {
    match cmd {
        VmDebugCommand::Boot(args) => run_boot(args).await,
    }
}

#[cfg(target_os = "macos")]
async fn run_boot(args: BootArgs) -> Result<()> {
    macos::run_boot(args).await
}

#[cfg(not(target_os = "macos"))]
async fn run_boot(args: BootArgs) -> Result<()> {
    // Validate inputs first so misconfigured invocations get a
    // useful "rootfs not found" / "kernel not found" message
    // even on platforms that can never actually boot — better
    // dev UX than failing on the platform check first when
    // the real problem is a bad path.
    validate_boot_inputs(&args)?;

    Err(anyhow!(
        "`elastos vm-debug boot` requires macOS on Apple Silicon \
         with Apple Virtualization.framework support. This host is \
         not macOS — see docs/MAC.md for the support boundary. \
         (The macOS path would have used boot args: '{}'.)",
        VM_DEBUG_DEFAULT_BOOT_ARGS
    ))
}

/// Validate the two on-disk inputs the boot command depends on.
///
/// Lifted out of the macOS-only path so unit tests can exercise
/// it on every platform — there is no reason to require Vz
/// linkage to assert "missing kernel returns a typed error".
pub(crate) fn validate_boot_inputs(args: &BootArgs) -> Result<()> {
    if !args.rootfs.exists() {
        bail!(
            "rootfs not found: {} (pass --rootfs /path/to/rootfs.img)",
            args.rootfs.display()
        );
    }
    if !args.kernel.exists() {
        bail!(
            "kernel not found: {} (pass --kernel /path/to/Image)",
            args.kernel.display()
        );
    }
    if args.memory_mb < 64 {
        bail!(
            "memory_mb must be >= 64 (got {}); Vz refuses tiny guests",
            args.memory_mb
        );
    }
    if args.vcpus == 0 {
        bail!("vcpus must be >= 1");
    }
    Ok(())
}

/// Lay out a fake capsule directory inside `staging_root` so
/// `VzProvider::load(&capsule_dir, manifest)` can find a rootfs
/// at `<capsule_dir>/<manifest.entrypoint>`. Pure path-only
/// helper — does not require Vz linkage, so the same logic is
/// covered by tests on Linux.
///
/// `cfg_attr(allow(dead_code))` on Linux because nothing on the
/// non-macOS path currently calls it (the Linux `run_boot`
/// shortcuts to the "macOS only" error after input validation).
/// The test in this module still exercises the function on
/// every platform, but tests don't influence the non-test
/// build's dead-code lint.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn build_staging_layout(
    staging_root: &std::path::Path,
    rootfs: &std::path::Path,
) -> Result<StagedCapsule> {
    let capsule_dir = staging_root.join("capsule");
    std::fs::create_dir_all(&capsule_dir).with_context(|| {
        format!(
            "vm-debug boot: cannot create capsule staging dir {}",
            capsule_dir.display()
        )
    })?;
    let rootfs_target = capsule_dir.join("rootfs.img");

    // Hard-link if possible (zero-cost). Falls back to a regular
    // copy if the user's rootfs is on a different filesystem, or
    // if hard-linking would corrupt the original (e.g. some FUSE
    // mounts). We never write through `rootfs_target` ourselves;
    // it's only the source path Vz reads.
    if std::fs::hard_link(rootfs, &rootfs_target).is_err() {
        std::fs::copy(rootfs, &rootfs_target).with_context(|| {
            format!(
                "vm-debug boot: failed to hard-link or copy {} into {}",
                rootfs.display(),
                rootfs_target.display()
            )
        })?;
    }

    Ok(StagedCapsule {
        capsule_dir,
        entrypoint: "rootfs.img".to_string(),
    })
}

/// Resolved staging-dir layout produced by [`build_staging_layout`].
/// Same reason as the fn for the non-macOS `allow(dead_code)`.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone)]
pub(crate) struct StagedCapsule {
    pub(crate) capsule_dir: PathBuf,
    pub(crate) entrypoint: String,
}

#[cfg(target_os = "macos")]
mod macos {
    use std::time::Duration;

    use anyhow::{Context, Result};
    use elastos_common::{
        CapsuleManifest, CapsuleRole, CapsuleStatus, CapsuleType, MicroVmConfig, ResourceLimits,
        SCHEMA_V1,
    };
    use elastos_compute::ComputeProvider;
    use elastos_vz::{VzConfig, VzProvider};

    use super::{build_staging_layout, validate_boot_inputs, BootArgs, VM_DEBUG_DEFAULT_BOOT_ARGS};

    pub(super) async fn run_boot(args: BootArgs) -> Result<()> {
        if !elastos_vz::is_supported() {
            anyhow::bail!(
                "Apple Virtualization.framework not available on this host. \
                 Requires macOS 12+ on Apple Silicon — see docs/MAC.md."
            );
        }

        validate_boot_inputs(&args)?;

        // The whole boot runs inside a tempdir so a Ctrl-C +
        // process exit leaves no per-VM state behind. The
        // identifier.bin persistence Phase 2 Day 2 added is
        // anchored to this dir; for vm-debug we WANT it
        // ephemeral.
        let tmp = tempfile::tempdir().context("vm-debug boot: cannot create tempdir")?;
        let staged = build_staging_layout(tmp.path(), &args.rootfs)?;

        let boot_args = args
            .boot_args
            .clone()
            .unwrap_or_else(|| VM_DEBUG_DEFAULT_BOOT_ARGS.to_string());

        let manifest = build_debug_manifest(&args, &staged.entrypoint, &boot_args);

        let kernel = args
            .kernel
            .canonicalize()
            .context("vm-debug boot: cannot canonicalize --kernel path")?;
        let vz_config = VzConfig::new()
            .with_state_dir(tmp.path().join("vz-state"))
            .with_rootfs_cache_dir(tmp.path().join("rootfs-cache"))
            .with_kernel_path(kernel);

        let provider = VzProvider::new(vz_config)
            .map_err(|e| anyhow::anyhow!("vm-debug boot: cannot construct VzProvider: {e}"))?;
        provider
            .init()
            .await
            .map_err(|e| anyhow::anyhow!("vm-debug boot: provider.init: {e}"))?;

        println!("vm-debug boot: loading capsule");
        let handle = provider
            .load(&staged.capsule_dir, manifest)
            .await
            .map_err(|e| anyhow::anyhow!("vm-debug boot: provider.load: {e}"))?;
        println!("vm-debug boot: capsule loaded ({}); starting…", handle.id);

        provider
            .start(&handle)
            .await
            .map_err(|e| anyhow::anyhow!("vm-debug boot: provider.start: {e}"))?;
        println!(
            "vm-debug boot: guest started. Press Ctrl-C to stop. \
             Guest kernel console streams via tracing target `vm_console`."
        );

        wait_for_guest_to_finish(&provider, &handle).await;

        println!("vm-debug boot: stopping VM…");
        if let Err(e) = provider.stop(&handle).await {
            // Stop is best-effort during teardown — we still
            // want the tempdir cleanup to run even if Vz
            // reports an error.
            eprintln!("vm-debug boot: provider.stop returned: {e}");
        }
        println!("vm-debug boot: done.");
        Ok(())
    }

    fn build_debug_manifest(args: &BootArgs, entrypoint: &str, boot_args: &str) -> CapsuleManifest {
        CapsuleManifest {
            schema: SCHEMA_V1.into(),
            version: "0.0.0-vm-debug".into(),
            name: "vm-debug-boot".into(),
            description: Some("Phase 2 Day 4 `elastos vm-debug boot` ephemeral capsule".into()),
            author: None,
            role: CapsuleRole::App,
            capsule_type: CapsuleType::MicroVM,
            entrypoint: entrypoint.into(),
            requires: Vec::new(),
            provides: None,
            capabilities: Vec::new(),
            resources: ResourceLimits {
                memory_mb: args.memory_mb,
                cpu_shares: 100,
                gpu: false,
            },
            permissions: Default::default(),
            microvm: Some(MicroVmConfig {
                kernel: None,
                boot_args: boot_args.into(),
                http_port: None,
                vcpu_count: Some(args.vcpus),
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

    /// Race Ctrl-C against the VM's own state. Returns when
    /// either fires — the caller is responsible for actually
    /// calling `provider.stop(...)` afterwards.
    async fn wait_for_guest_to_finish(
        provider: &VzProvider,
        handle: &elastos_compute::CapsuleHandle,
    ) {
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    println!();
                    println!("vm-debug boot: Ctrl-C received");
                    return;
                }
                _ = tokio::time::sleep(Duration::from_millis(500)) => {
                    match provider.status(handle).await {
                        Ok(CapsuleStatus::Running) => continue,
                        Ok(other) => {
                            println!("vm-debug boot: guest state transitioned to {other:?}");
                            return;
                        }
                        Err(e) => {
                            eprintln!("vm-debug boot: status query failed: {e}");
                            return;
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(rootfs: PathBuf, kernel: PathBuf) -> BootArgs {
        BootArgs {
            rootfs,
            kernel,
            memory_mb: 256,
            vcpus: 1,
            boot_args: None,
        }
    }

    #[test]
    fn validate_boot_inputs_rejects_missing_rootfs() {
        let tmp = tempfile::tempdir().unwrap();
        let kernel = tmp.path().join("Image");
        std::fs::write(&kernel, b"k").unwrap();
        let rootfs = tmp.path().join("does-not-exist.img");

        let err = validate_boot_inputs(&args(rootfs, kernel)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("rootfs not found"),
            "expected 'rootfs not found', got: {msg}"
        );
    }

    #[test]
    fn validate_boot_inputs_rejects_missing_kernel() {
        let tmp = tempfile::tempdir().unwrap();
        let rootfs = tmp.path().join("rootfs.img");
        std::fs::write(&rootfs, b"r").unwrap();
        let kernel = tmp.path().join("missing-Image");

        let err = validate_boot_inputs(&args(rootfs, kernel)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("kernel not found"),
            "expected 'kernel not found', got: {msg}"
        );
    }

    #[test]
    fn validate_boot_inputs_rejects_tiny_memory() {
        let tmp = tempfile::tempdir().unwrap();
        let rootfs = tmp.path().join("r");
        let kernel = tmp.path().join("k");
        std::fs::write(&rootfs, b"r").unwrap();
        std::fs::write(&kernel, b"k").unwrap();
        let mut a = args(rootfs, kernel);
        a.memory_mb = 32;

        let err = validate_boot_inputs(&a).unwrap_err();
        assert!(err.to_string().contains("memory_mb must be >= 64"));
    }

    #[test]
    fn validate_boot_inputs_rejects_zero_vcpus() {
        let tmp = tempfile::tempdir().unwrap();
        let rootfs = tmp.path().join("r");
        let kernel = tmp.path().join("k");
        std::fs::write(&rootfs, b"r").unwrap();
        std::fs::write(&kernel, b"k").unwrap();
        let mut a = args(rootfs, kernel);
        a.vcpus = 0;

        let err = validate_boot_inputs(&a).unwrap_err();
        assert!(err.to_string().contains("vcpus must be >= 1"));
    }

    #[test]
    fn build_staging_layout_creates_capsule_dir_with_entrypoint_rootfs() {
        let tmp = tempfile::tempdir().unwrap();
        let rootfs = tmp.path().join("real-rootfs.img");
        std::fs::write(&rootfs, b"fake-rootfs-bytes").unwrap();

        let staging_root = tmp.path().join("staging");
        std::fs::create_dir_all(&staging_root).unwrap();

        let staged = build_staging_layout(&staging_root, &rootfs).unwrap();

        // Layout must match what VzProvider::load expects.
        assert!(staged.capsule_dir.is_dir());
        assert_eq!(staged.entrypoint, "rootfs.img");
        let staged_rootfs = staged.capsule_dir.join(&staged.entrypoint);
        assert!(staged_rootfs.is_file());
        // The staged file must carry the original bytes — Vz
        // reads from this path verbatim.
        assert_eq!(std::fs::read(&staged_rootfs).unwrap(), b"fake-rootfs-bytes");
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn vm_debug_boot_on_non_macos_returns_typed_error_pointing_at_docs() {
        let tmp = tempfile::tempdir().unwrap();
        let rootfs = tmp.path().join("rootfs.img");
        let kernel = tmp.path().join("Image");
        std::fs::write(&rootfs, b"r").unwrap();
        std::fs::write(&kernel, b"k").unwrap();

        let cmd = VmDebugCommand::Boot(args(rootfs, kernel));
        let err = run(cmd).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("Apple Virtualization.framework"),
            "expected Apple Virtualization marker, got: {msg}"
        );
        assert!(
            msg.contains("docs/MAC.md"),
            "expected docs/MAC.md anchor, got: {msg}"
        );
    }
}
