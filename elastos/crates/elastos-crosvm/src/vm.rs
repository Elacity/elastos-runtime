//! Running VM lifecycle management (crosvm)

use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use elastos_common::{CapsuleManifest, CapsuleStatus, ElastosError, Result};
use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;
use tokio::process::{Child, Command};

use crate::config::VmConfig;

/// Owned private boot input. Remove only the inode this VM created.
struct BootParams {
    path: PathBuf,
    device: u64,
    inode: u64,
}

impl BootParams {
    fn create(directory: &Path, params: &str) -> std::io::Result<Self> {
        if params.len() > 65536 || params.contains('\0') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid private kernel parameters",
            ));
        }
        let path = directory.join(format!("boot-{}.params", uuid::Uuid::new_v4()));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let metadata = file.metadata()?;
        let owned = Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
        };
        file.write_all(params.as_bytes())?;
        file.sync_all()?;
        Ok(owned)
    }
}

impl Drop for BootParams {
    fn drop(&mut self) {
        if let Ok(metadata) = std::fs::symlink_metadata(&self.path) {
            if metadata.is_file() && metadata.dev() == self.device && metadata.ino() == self.inode {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

fn redact_guest_console(line: &str) -> String {
    line.split_whitespace()
        .map(|word| {
            if word.starts_with("elastos.token=") {
                "elastos.token=[private]"
            } else if word.starts_with("elastos.browser_ice_config_hex=") {
                "elastos.browser_ice_config_hex=[private]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A running crosvm VM instance
pub struct RunningVm {
    /// VM configuration
    pub config: VmConfig,

    /// Capsule manifest
    pub manifest: CapsuleManifest,

    /// Current status
    pub status: CapsuleStatus,

    /// crosvm control socket path (for `crosvm stop`)
    pub socket_path: PathBuf,

    /// crosvm process handle
    process: Option<Child>,

    /// Retained until the child exits, or this owner is dropped.
    boot_params: Option<BootParams>,

    /// Process ID (for signal sending)
    pid: Option<u32>,

    /// Path to crosvm binary (for `crosvm stop`)
    crosvm_bin: Option<PathBuf>,
}

impl RunningVm {
    /// Create a new VM (not yet started)
    pub fn new(config: VmConfig, manifest: CapsuleManifest, socket_path: PathBuf) -> Self {
        Self {
            config,
            manifest,
            status: CapsuleStatus::Loading,
            socket_path,
            process: None,
            boot_params: None,
            pid: None,
            crosvm_bin: None,
        }
    }

    /// Start the VM using crosvm
    pub async fn start(&mut self, crosvm_bin: &std::path::Path) -> Result<()> {
        if self.process.is_some() {
            return Err(ElastosError::Compute("VM already started".into()));
        }

        // Require KVM — no fallback
        if !std::path::Path::new("/dev/kvm").exists() {
            return Err(ElastosError::Compute(
                "/dev/kvm not available — crosvm requires KVM".into(),
            ));
        }

        // Guest networking is explicit compatibility mode only.
        // Ordinary app capsules use the serial Carrier bridge and no guest NIC.

        // Ensure socket directory exists
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                ElastosError::Compute(format!("Failed to create socket dir: {}", e))
            })?;
        }

        // Ensure crosvm pivot-root directory exists (avoids host /var/empty dependency).
        if let Some(ref pivot_root) = self.config.pivot_root_dir {
            tokio::fs::create_dir_all(pivot_root).await.map_err(|e| {
                ElastosError::Compute(format!(
                    "Failed to create crosvm pivot-root dir '{}': {}",
                    pivot_root.display(),
                    e
                ))
            })?;
        }

        if let Some(ref network) = self.config.network {
            tracing::info!(
                "Setting up guest-network TAP for VM '{}': tap={} host={} guest={}",
                self.manifest.name,
                network.tap_name,
                network.host_ip,
                network.guest_ip
            );
            network.setup().map_err(|e| {
                ElastosError::Compute(format!(
                    "guest-network TAP setup failed for '{}': {}",
                    self.manifest.name, e
                ))
            })?;
        }

        // Ensure socket directory exists and is writable.
        // setcap binaries run with AT_SECURE which may restrict /tmp access.
        if let Some(parent) = self.socket_path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        let _ = tokio::fs::remove_file(&self.socket_path).await;

        // Build crosvm run command
        let mut crosvm_args = self.config.to_crosvm_args();
        let boot_params = BootParams::create(
            self.socket_path.parent().unwrap_or_else(|| Path::new(".")),
            &self.config.boot_args,
        )
        .map_err(|e| ElastosError::Compute(format!("private kernel parameter file failed: {e}")))?;
        let kernel_index = crosvm_args.len() - 1;
        crosvm_args.splice(
            kernel_index..kernel_index,
            [
                "--params-file".into(),
                boot_params.path.to_string_lossy().into_owned(),
            ],
        );

        tracing::info!(
            "Starting crosvm VM '{}': {} run --socket {} {}",
            self.manifest.name,
            crosvm_bin.display(),
            self.socket_path.display(),
            crosvm_args.join(" ")
        );

        // Spawn: crosvm run --socket <path> [vm args...]
        let mut command = Command::new(crosvm_bin);
        command
            .arg("run")
            .arg("--socket")
            .arg(&self.socket_path)
            .args(&crosvm_args)
            .kill_on_drop(true);

        if self.config.interactive_stdio {
            command
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit());
        } else {
            command
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
        }

        let mut child = command.spawn().map_err(|e| {
            if let Some(ref network) = self.config.network {
                let _ = network.teardown();
            }
            ElastosError::Compute(format!("Failed to start crosvm: {}", e))
        })?;

        // Forward VM serial output through tracing.
        if !self.config.interactive_stdio {
            if let Some(stdout) = child.stdout.take() {
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let reader = tokio::io::BufReader::new(stdout);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::info!(target: "vm_console", "{}", redact_guest_console(&line));
                    }
                });
            }
            if let Some(stderr) = child.stderr.take() {
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let reader = tokio::io::BufReader::new(stderr);
                    let mut lines = reader.lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::warn!(target: "vm_console", "{}", redact_guest_console(&line));
                    }
                });
            }
        }

        self.pid = child.id();
        self.process = Some(child);
        self.boot_params = Some(boot_params);
        self.status = CapsuleStatus::Running;
        self.crosvm_bin = Some(crosvm_bin.to_path_buf());

        tracing::info!(
            "crosvm VM '{}' started (pid: {:?})",
            self.manifest.name,
            self.pid,
        );

        Ok(())
    }

    /// Stop the VM gracefully (via `crosvm stop` or SIGTERM)
    pub async fn stop(&mut self) -> Result<()> {
        // Try `crosvm stop` via control socket first
        let crosvm = self
            .crosvm_bin
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("crosvm"));
        if self.socket_path.exists() {
            let output = tokio::process::Command::new(crosvm)
                .arg("stop")
                .arg(&self.socket_path)
                .output()
                .await;

            match output {
                Ok(o) if o.status.success() => {
                    tracing::info!(
                        "crosvm VM '{}' stopped via control socket",
                        self.manifest.name
                    );
                }
                _ => {
                    // Fallback to SIGTERM
                    if let Some(pid) = self.pid {
                        let nix_pid = Pid::from_raw(pid as i32);
                        if let Err(e) = signal::kill(nix_pid, Signal::SIGTERM) {
                            tracing::warn!("Failed to send SIGTERM to VM: {}", e);
                        }
                    }
                }
            }
        } else if let Some(pid) = self.pid {
            let nix_pid = Pid::from_raw(pid as i32);
            if let Err(e) = signal::kill(nix_pid, Signal::SIGTERM) {
                tracing::warn!("Failed to send SIGTERM to VM: {}", e);
            }
        }

        // Wait for process to exit
        if let Some(ref mut child) = self.process {
            match tokio::time::timeout(std::time::Duration::from_secs(10), child.wait()).await {
                Ok(Ok(status)) => {
                    tracing::info!(
                        "crosvm VM '{}' exited with status: {}",
                        self.manifest.name,
                        status
                    );
                }
                Ok(Err(e)) => {
                    tracing::warn!("Error waiting for VM to exit: {}", e);
                }
                Err(_) => {
                    tracing::warn!("VM did not exit within timeout, killing");
                    self.kill().await?;
                }
            }
        }

        self.process = None;
        self.boot_params = None;
        self.pid = None;
        self.status = CapsuleStatus::Stopped;

        // Clean up socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        if let Some(ref network) = self.config.network {
            let _ = network.teardown();
        }

        Ok(())
    }

    /// Force kill the VM
    pub async fn kill(&mut self) -> Result<()> {
        if let Some(pid) = self.pid {
            let nix_pid = Pid::from_raw(pid as i32);
            if let Err(e) = signal::kill(nix_pid, Signal::SIGKILL) {
                tracing::warn!("Failed to send SIGKILL to VM: {}", e);
            }
        }

        if let Some(ref mut child) = self.process {
            let _ = child.wait().await;
        }

        self.process = None;
        self.boot_params = None;
        self.pid = None;
        self.status = CapsuleStatus::Stopped;

        // Clean up socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        if let Some(ref network) = self.config.network {
            let _ = network.teardown();
        }

        Ok(())
    }

    /// Check if the VM is running
    /// Wait for the VM process to exit. Returns the exit status.
    pub async fn wait_for_exit(&mut self) -> Result<std::process::ExitStatus> {
        let status = match self.process.as_mut() {
            Some(child) => child
                .wait()
                .await
                .map_err(|e| ElastosError::Compute(format!("wait failed: {}", e)))?,
            None => return Err(ElastosError::Compute("no VM process to wait on".into())),
        };
        self.process = None;
        self.pid = None;
        self.boot_params = None;
        self.status = CapsuleStatus::Stopped;
        Ok(status)
    }

    pub fn is_running(&self) -> bool {
        if let Some(pid) = self.pid {
            let nix_pid = Pid::from_raw(pid as i32);
            signal::kill(nix_pid, None).is_ok()
        } else {
            false
        }
    }

    /// Get the VM's HTTP port (if configured)
    pub fn http_port(&self) -> Option<u16> {
        self.config.http_port
    }

    /// Get the VM's process ID
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }
}

impl Drop for RunningVm {
    fn drop(&mut self) {
        // Try to clean up the VM if it's still running
        if let Some(pid) = self.pid {
            let nix_pid = Pid::from_raw(pid as i32);
            let _ = signal::kill(nix_pid, Signal::SIGKILL);
        }
        if let Some(ref network) = self.config.network {
            let _ = network.teardown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_boot_params_are_bounded_mode_600_and_owned_until_cleanup() {
        let directory = tempfile::tempdir().unwrap();
        let params =
            "console=ttyS0 elastos.token=test-private-token elastos.browser_ice_config_hex=abcdef";
        let owned = BootParams::create(directory.path(), params).unwrap();
        let path = owned.path.clone();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), params);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(owned);
        assert!(!path.exists());
        assert!(BootParams::create(directory.path(), &"x".repeat(65537)).is_err());
        assert!(BootParams::create(directory.path(), "x\0y").is_err());
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    }

    #[test]
    fn private_boot_cleanup_preserves_a_replacement_inode() {
        let directory = tempfile::tempdir().unwrap();
        let owned = BootParams::create(directory.path(), "private").unwrap();
        let path = owned.path.clone();
        let original = directory.path().join("original");
        std::fs::rename(&path, &original).unwrap();
        std::fs::write(&path, "replacement").unwrap();
        drop(owned);
        assert_eq!(std::fs::read_to_string(path).unwrap(), "replacement");
    }

    #[test]
    fn kernel_console_keeps_private_boot_values_out_of_logs() {
        assert_eq!(
            redact_guest_console("Kernel command line: console=ttyS0 elastos.token=test-private-token elastos.browser_ice_config_hex=abcdef"),
            "Kernel command line: console=ttyS0 elastos.token=[private] elastos.browser_ice_config_hex=[private]"
        );
    }
}
