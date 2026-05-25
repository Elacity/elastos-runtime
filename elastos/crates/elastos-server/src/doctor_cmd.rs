//! `elastos doctor` — substrate path resolution inspector.
//!
//! Read-only triage command that constructs the same [`Supervisor`] the
//! runtime daemon would (against the current `data_dir` + `components.json`)
//! and prints the resolved Vz substrate paths it intends to load — kernel,
//! initrd, state_dir, rootfs_cache_dir. Each row reports presence and, where
//! applicable, runs [`elastos_vz::VzConfig::validate`] for a guest-kernel
//! sanity check. Absent artifacts come with a one-line remediation hint
//! (`elastos setup --profile minimal`).
//!
//! Motivation: Phase 7 Day 3 closed a latent data-dir mismatch between
//! `elastos-vz` (Unix-style `~/.local/share/elastos`) and the
//! `elastos-server` installer (macOS `~/Library/Application Support/elastos`).
//! That class of "the file is staged but the substrate is looking at the
//! other directory" bug is precisely what doctor is designed to surface
//! before users hit a `KernelNotFound` at launch time.
//!
//! The command is pure (no on-disk mutations). Output is rendered through
//! a `&mut dyn Write` so unit tests can capture it into a `Vec<u8>` and
//! assert on substrings without spawning a subprocess.

use std::io::Write;
use std::path::Path;

use crate::setup::{detect_platform, load_manifest, ComponentsManifest, PlatformInfo};
use crate::supervisor::Supervisor;

/// CLI arguments for `elastos doctor`.
#[derive(Debug, Clone, Default)]
pub struct DoctorArgs {
    /// When true, augment each substrate row with the manifest entry's
    /// URL / checksum / size / compression. Off by default to keep the
    /// summary triage-friendly.
    pub verbose: bool,
}

/// Top-level entry point. Resolves the live `data_dir` + manifest from disk
/// and writes the report to stdout.
///
/// Phase 7 Day 6 — the body runs inside `tracing::subscriber::with_default`
/// with a WARN-and-above subscriber, so the supervisor's INFO logs
/// (e.g. `vz: startup orphan-prune complete`) do not bleed into the
/// inspector output. `doctor` is a one-shot triage CLI; the global
/// `elastos=info` subscriber installed in `main` is appropriate for
/// long-running `serve`/`setup` paths but distracting here. The
/// `with_default` swap is thread-local and ends when this function
/// returns — other subcommands keep the operator's `RUST_LOG`
/// configuration intact. Safe because the body of `run` is purely
/// synchronous (no `.await`), so no task migration can leak the
/// override across worker threads.
pub async fn run(args: DoctorArgs) -> anyhow::Result<()> {
    tracing::subscriber::with_default(build_quiet_subscriber(), || -> anyhow::Result<()> {
        let data_dir = crate::sources::default_data_dir();
        let manifest = load_manifest()?;
        let platform = detect_platform();

        print_report(
            &mut std::io::stdout(),
            &data_dir,
            &manifest,
            &platform,
            args.verbose,
        )
    })
}

/// Build the WARN-and-above fmt subscriber installed by [`run`].
/// Extracted as a free function so the unit test can install the
/// same subscriber against a capture-buffer `MakeWriter` and assert
/// the suppression behaviour directly.
fn build_quiet_subscriber() -> impl tracing::Subscriber + Send + Sync {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .finish()
}

/// Write the substrate report to `out`. Extracted from [`run`] so unit
/// tests can drive it with a synthetic `data_dir` + manifest and capture
/// the output into a buffer.
pub(crate) fn print_report(
    out: &mut dyn Write,
    data_dir: &Path,
    manifest: &ComponentsManifest,
    platform: &str,
    verbose: bool,
) -> anyhow::Result<()> {
    writeln!(out, "ElastOS doctor — substrate path resolution check")?;
    writeln!(out, "  platform:   {platform}")?;
    writeln!(out, "  data_dir:   {}", data_dir.display())?;
    writeln!(out)?;

    // Construct the same supervisor `serve_cmd` would, then read back the
    // resolved Vz paths. This is the entire point of doctor: ground-truth
    // the paths the substrate will actually try to load, not the paths a
    // human guesses from the manifest.
    let supervisor = Supervisor::new(data_dir.to_path_buf(), manifest.clone());
    let vz_config = supervisor.vz_config();

    print_artifact_row(
        out,
        ArtifactRow {
            label: "vmlinux",
            path: &vz_config.kernel_path,
            validate_as_kernel: true,
            remediation: "elastos setup --profile minimal",
            manifest_component: "vmlinux",
        },
        manifest,
        verbose,
    )?;

    match vz_config.initramfs_path.as_deref() {
        Some(path) => {
            print_artifact_row(
                out,
                ArtifactRow {
                    label: "initrd",
                    path,
                    validate_as_kernel: false,
                    remediation: "elastos setup --profile minimal",
                    manifest_component: "initrd",
                },
                manifest,
                verbose,
            )?;
        }
        None => {
            writeln!(out, "  initrd:     not configured")?;
            writeln!(
                out,
                "              kernel-only boot path (only valid if vmlinux has built-in virtio drivers)"
            )?;
            writeln!(out)?;
        }
    }

    print_dir_row(out, "state_dir", &vz_config.state_dir)?;
    print_dir_row(out, "rootfs_cache_dir", &vz_config.rootfs_cache_dir)?;

    Ok(())
}

/// Parameters for a single substrate-artifact row. Bundled in a struct
/// to keep [`print_artifact_row`]'s signature readable (clippy
/// `too_many_arguments`) and to make adding future rows mechanical.
struct ArtifactRow<'a> {
    label: &'a str,
    path: &'a Path,
    validate_as_kernel: bool,
    remediation: &'a str,
    manifest_component: &'a str,
}

fn print_artifact_row(
    out: &mut dyn Write,
    row: ArtifactRow<'_>,
    manifest: &ComponentsManifest,
    verbose: bool,
) -> anyhow::Result<()> {
    writeln!(out, "  {}:     {}", row.label, row.path.display())?;

    if !row.path.exists() {
        writeln!(out, "              [absent]")?;
        writeln!(out, "              → run: {}", row.remediation)?;
    } else if !row.path.is_file() {
        writeln!(
            out,
            "              [present but not a regular file] expected a file at this path"
        )?;
        writeln!(out, "              → run: {}", row.remediation)?;
    } else {
        let size = std::fs::metadata(row.path).map(|m| m.len()).unwrap_or(0);
        writeln!(out, "              [present] size {}", human_bytes(size))?;

        if row.validate_as_kernel {
            // Synthesize a probe `VzConfig` aimed at this exact kernel
            // path so we can leverage the substrate's own validator
            // without bringing the supervisor's launch path online.
            let probe = elastos_vz::VzConfig::new().with_kernel_path(row.path);
            match probe.validate() {
                Ok(()) => writeln!(
                    out,
                    "              [validate] passes guest-kernel sanity check"
                )?,
                Err(e) => {
                    writeln!(out, "              [validate FAIL] {e}")?;
                    writeln!(out, "              → run: {}", row.remediation)?;
                }
            }
        }
    }

    if verbose {
        if let Some(info) = current_platform_info(manifest, row.manifest_component) {
            print_manifest_metadata(out, info)?;
        }
    }

    writeln!(out)?;
    Ok(())
}

fn print_dir_row(out: &mut dyn Write, label: &str, path: &Path) -> anyhow::Result<()> {
    writeln!(out, "  {label}:  {}", path.display())?;
    if path.is_dir() {
        writeln!(out, "              [present]")?;
    } else if path.exists() {
        writeln!(out, "              [exists but not a directory]")?;
    } else {
        // Directories are lazily created at first launch by the substrate,
        // so absence is informational, not an error.
        writeln!(
            out,
            "              [absent — will be created on first launch]"
        )?;
    }
    writeln!(out)?;
    Ok(())
}

fn current_platform_info<'a>(
    manifest: &'a ComponentsManifest,
    component_name: &str,
) -> Option<&'a PlatformInfo> {
    let component = manifest.external.get(component_name)?;
    component.platforms.get(&detect_platform())
}

fn print_manifest_metadata(out: &mut dyn Write, info: &PlatformInfo) -> anyhow::Result<()> {
    if let Some(url) = &info.url {
        writeln!(out, "              url:         {url}")?;
    }
    if let Some(cs) = &info.checksum {
        writeln!(out, "              checksum:    {cs}")?;
    }
    if let Some(c) = &info.compression {
        writeln!(out, "              compression: {c}")?;
    }
    if let Some(s) = info.size {
        writeln!(out, "              manifest-size: {s} bytes")?;
    }
    Ok(())
}

/// Human-readable byte formatter used in row output. Kept private — there
/// is already a project-wide formatter in setup.rs, but it's wired through
/// a different output column convention. Local copy keeps doctor's output
/// stable independent of setup's UI churn.
fn human_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    if n >= GB {
        format!("{:.1} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.1} KB", n as f64 / KB as f64)
    } else {
        format!("{n} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::Component;
    use std::collections::HashMap;

    fn fixture_manifest() -> ComponentsManifest {
        // Build a minimal manifest that matches what `Supervisor::new`
        // expects: an `external.vmlinux` entry with a platform row for
        // the test runner's platform that resolves to `bin/vmlinux`.
        let mut platforms = HashMap::new();
        platforms.insert(
            detect_platform(),
            PlatformInfo {
                url: Some("https://example.test/vmlinux".to_string()),
                cid: None,
                release_path: None,
                checksum: Some("sha256:0123456789abcdef".to_string()),
                extract_path: None,
                install_path: Some("bin/vmlinux".to_string()),
                strategy: None,
                source: None,
                note: None,
                size: Some(1234),
                compression: Some("gzip".to_string()),
            },
        );
        let mut external = HashMap::new();
        external.insert(
            "vmlinux".to_string(),
            Component {
                version: None,
                install_path: Some("bin/vmlinux".to_string()),
                size_mb: None,
                description: None,
                platforms,
            },
        );

        ComponentsManifest {
            external,
            capsules: HashMap::new(),
            profiles: HashMap::new(),
        }
    }

    fn report(data_dir: &Path, manifest: &ComponentsManifest, verbose: bool) -> String {
        let mut buf = Vec::new();
        print_report(&mut buf, data_dir, manifest, "test-platform", verbose).unwrap();
        String::from_utf8(buf).expect("report output is utf-8")
    }

    #[test]
    fn doctor_reports_absent_artifact_with_remediation() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        let manifest = fixture_manifest();

        let out = report(data_dir, &manifest, false);

        assert!(
            out.contains("vmlinux:"),
            "expected a vmlinux row, got:\n{out}"
        );
        assert!(
            out.contains("[absent]"),
            "expected [absent] tag, got:\n{out}"
        );
        assert!(
            out.contains("elastos setup --profile minimal"),
            "expected remediation hint, got:\n{out}"
        );
        // state_dir / rootfs_cache_dir rows are still rendered even with
        // no on-disk artifacts.
        assert!(
            out.contains("state_dir:"),
            "expected state_dir row, got:\n{out}"
        );
        assert!(
            out.contains("rootfs_cache_dir:"),
            "expected rootfs_cache_dir row, got:\n{out}"
        );
    }

    /// Phase 7 Day 6 — verify the quiet-subscriber pattern actually
    /// suppresses INFO-level tracing events while letting WARN/ERROR
    /// through. We re-build the same subscriber [`run`] installs, but
    /// point its writer at an `Arc<Mutex<Vec<u8>>>` capture buffer so
    /// the assertion is over what an operator would have seen on
    /// stderr — not over implementation details like `max_level_hint`.
    #[test]
    fn doctor_quiet_subscriber_suppresses_info_logs_but_passes_warn() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        /// Minimal `MakeWriter` adapter over a shared byte buffer so
        /// the test can assert on the formatted log lines the
        /// subscriber would have emitted.
        #[derive(Clone)]
        struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

        struct CaptureHandle(Arc<Mutex<Vec<u8>>>);

        impl Write for CaptureHandle {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        impl<'a> MakeWriter<'a> for CaptureWriter {
            type Writer = CaptureHandle;
            fn make_writer(&'a self) -> Self::Writer {
                CaptureHandle(self.0.clone())
            }
        }

        let buffer: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let capture = CaptureWriter(buffer.clone());

        // Mirror exactly what `build_quiet_subscriber` does, but with
        // the capture writer instead of stderr. Any divergence in the
        // shape of this subscriber from `build_quiet_subscriber` would
        // be a test-only false negative, so this construction stays
        // intentionally tight.
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::WARN)
            .with_writer(capture)
            .with_ansi(false)
            .finish();

        tracing::subscriber::with_default(subscriber, || {
            // Mimic the supervisor's startup INFO line that bleeds
            // into pre-Day-6 doctor output.
            tracing::info!("vz: startup orphan-prune complete");
            // A real warning that operators should still see.
            tracing::warn!("doctor: substrate kernel checksum mismatch");
        });

        let captured = String::from_utf8(buffer.lock().unwrap().clone())
            .expect("capture buffer should be valid utf-8");

        assert!(
            !captured.contains("startup orphan-prune"),
            "Phase 7 Day 6: WARN subscriber should suppress INFO events, \
             but the captured output contained the supervisor's startup \
             INFO marker. Captured:\n{captured}"
        );
        assert!(
            captured.contains("substrate kernel checksum mismatch"),
            "Phase 7 Day 6: WARN-and-above events must still pass through \
             — otherwise doctor would silently hide real problems. \
             Captured:\n{captured}"
        );
    }

    #[test]
    fn doctor_reports_present_artifact_with_size_and_verbose_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let data_dir = tmp.path();
        std::fs::create_dir_all(data_dir.join("bin")).unwrap();
        // Any non-empty placeholder is enough for the [present] branch.
        // The kernel-signature validator will fail on this — we assert
        // both [present] AND that the validate-FAIL message is rendered,
        // which is the doctor surface that catches a corrupted artifact
        // staged at the right path.
        std::fs::write(data_dir.join("bin/vmlinux"), b"placeholder-not-a-real-kernel").unwrap();

        let manifest = fixture_manifest();
        let out = report(data_dir, &manifest, true);

        assert!(
            out.contains("[present]"),
            "expected [present] tag, got:\n{out}"
        );
        assert!(
            out.contains("[validate FAIL]"),
            "expected guest-kernel validator to reject placeholder bytes, got:\n{out}"
        );
        // Verbose mode should echo the manifest URL/checksum/compression.
        assert!(
            out.contains("https://example.test/vmlinux"),
            "expected verbose URL line, got:\n{out}"
        );
        assert!(
            out.contains("compression: gzip"),
            "expected verbose compression line, got:\n{out}"
        );
    }
}
