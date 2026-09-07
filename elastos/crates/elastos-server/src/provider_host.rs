//! Standalone native-provider hosting for `elastos run <provider>`.
//!
//! A native provider (`execution: "native-provider"` in `components.json`) has
//! no `capsule.json` and speaks the stdio provider protocol, so it cannot be
//! exec'd like an app capsule — it needs the runtime's core planes underneath
//! it: an identity, a provider registry, the in-process content plane, and a
//! Carrier node whose inbound `provider_invoke` frames dispatch into that
//! registry. This module owns that composition; `run_cmd` only routes to it.
//!
//! Deliberately absent: any HTTP listener, TLS material, session registry, or
//! capability manager. A provider host serves peers over Carrier and nothing
//! else.

use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context as _;
use elastos_runtime::provider::ProviderRegistry;

use crate::server_infra;

/// Where a running provider host publishes its readiness receipt.
const READY_RECEIPT_RELATIVE_PATH: &str = "run/provider-host.ready.json";

/// The remedy for a custody host whose state root has not been provisioned.
const CUSTODY_PROVISIONING_REMEDY: &str =
    "run `elastos protected-content-config provision-custody-node` on this home first";

/// The remedy for a chain plane without a protected-content network config.
/// A custody committee member settles every release through its own chain
/// rights evidence (`protected_content_rights_evidence`), so a host that
/// serves `custody` without `chain` fails every release closed.
const CHAIN_CONFIG_REMEDY: &str = "install the protected-content chain configuration at \
     <data_dir>/protected-content/chain-provider.json (the client's network, with evidence RPC \
     URLs reachable from this host) first";

/// A native provider this host knows how to stand up.
///
/// One variant per registration routine: adding a future provider (a
/// standalone dKMS node, say) is one variant plus one `register` arm, and the
/// exhaustive match makes forgetting either a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProviderPlane {
    /// Runtime-only `custody` target: the custody committee member peers dial.
    Custody,
    /// `elastos://availability` — replica placement and attestation.
    Availability,
    /// `elastos://ipfs` — the block backend the content plane pins through.
    Ipfs,
    /// `elastos://chain` — the rights-evidence oracle a custody committee
    /// member settles releases against (protected-content network config).
    Chain,
}

impl ProviderPlane {
    /// Every plane, in the order a host registers them. Custody first so an
    /// unprovisioned committee member fails before anything else is spawned.
    const ALL: &'static [ProviderPlane] = &[
        ProviderPlane::Custody,
        ProviderPlane::Availability,
        ProviderPlane::Ipfs,
        ProviderPlane::Chain,
    ];

    /// The installed component name, which is also the binary's file name.
    const fn provider_name(self) -> &'static str {
        match self {
            ProviderPlane::Custody => "custody-provider",
            ProviderPlane::Availability => "availability-provider",
            ProviderPlane::Ipfs => "ipfs-provider",
            ProviderPlane::Chain => "chain-provider",
        }
    }

    fn from_provider_name(name: &str) -> Option<Self> {
        ProviderPlane::ALL
            .iter()
            .copied()
            .find(|plane| plane.provider_name() == name)
    }

    pub(crate) fn supported_set() -> String {
        ProviderPlane::ALL
            .iter()
            .map(|plane| plane.provider_name())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Fail closed on prerequisites this plane needs on disk, naming the
    /// command that creates them. Runs before anything is spawned.
    fn check_prerequisites(self, data_dir: &Path) -> anyhow::Result<()> {
        match self {
            ProviderPlane::Custody => {
                let state_root =
                    elastos_server::protected_content_runtime::inactive_custody_state_root(
                        data_dir,
                    );
                if !state_root.is_dir() {
                    anyhow::bail!(
                        "custody-provider has no provisioned custody state at {}: {}",
                        state_root.display(),
                        CUSTODY_PROVISIONING_REMEDY
                    );
                }
                Ok(())
            }
            ProviderPlane::Chain => {
                let config = elastos_server::protected_content_runtime::load_runtime_protected_content_chain_provider_config(
                    data_dir,
                )
                .with_context(|| format!("chain-provider configuration is invalid: {CHAIN_CONFIG_REMEDY}"))?;
                if config.is_none() {
                    anyhow::bail!(
                        "chain-provider has no protected-content network configuration: {CHAIN_CONFIG_REMEDY}"
                    );
                }
                Ok(())
            }
            ProviderPlane::Availability | ProviderPlane::Ipfs => Ok(()),
        }
    }

    /// Spawn the provider capsule and publish it on its registry target.
    async fn register(
        self,
        registry: &Arc<ProviderRegistry>,
        binary_path: &Path,
        data_dir: &Path,
    ) -> anyhow::Result<()> {
        tracing::debug!(
            provider = self.provider_name(),
            binary = %binary_path.display(),
            "spawning native provider capsule"
        );
        match self {
            ProviderPlane::Custody => {
                // The unprovisioned case is already named precisely by
                // `check_prerequisites`; anything left here is a spawn or
                // handshake failure that must speak for itself.
                elastos_server::protected_content_runtime::register_inactive_custody_provider(
                    registry,
                    binary_path,
                    data_dir,
                )
                .await
            }
            ProviderPlane::Availability => {
                server_infra::register_availability_provider_plane(
                    registry,
                    binary_path,
                    server_infra::availability_provider_config_from_env()
                        .unwrap_or(serde_json::Value::Null),
                )
                .await
            }
            ProviderPlane::Ipfs => {
                server_infra::register_ipfs_provider_plane(registry, binary_path).await
            }
            ProviderPlane::Chain => {
                server_infra::register_chain_provider_plane(registry, binary_path, data_dir).await
            }
        }
    }

    /// The registry target this plane answers on, for the readiness log line.
    const fn registry_target(self) -> &'static str {
        match self {
            ProviderPlane::Custody => "custody",
            ProviderPlane::Availability => "availability",
            ProviderPlane::Ipfs => "ipfs",
            ProviderPlane::Chain => "chain",
        }
    }
}

/// One resolved provider: which plane, and the binary that implements it.
#[derive(Clone, Debug)]
struct HostedProvider {
    plane: ProviderPlane,
    binary_path: PathBuf,
}

/// A fully resolved provider-host composition. Constructing one proves every
/// named provider is supported and every binary was found, so `compose` has no
/// resolution failures left to handle.
#[derive(Clone, Debug)]
pub(crate) struct ProviderHostPlan {
    data_dir: PathBuf,
    providers: Vec<HostedProvider>,
    carrier_addr: Option<SocketAddr>,
}

impl ProviderHostPlan {
    /// Resolve `elastos run <target> [--with <extra>]… [--carrier-addr <addr>]`.
    ///
    /// Each target is either an installed provider name (`custody-provider`)
    /// or a path to a provider binary, whose file name names the plane.
    pub(crate) fn resolve(
        data_dir: PathBuf,
        target: &str,
        additional: &[String],
        carrier_addr: Option<&str>,
    ) -> anyhow::Result<Self> {
        let carrier_addr = carrier_addr
            .map(|addr| {
                addr.parse::<SocketAddr>().with_context(|| {
                    format!(
                        "--carrier-addr must be a socket address like 0.0.0.0:4433, got '{addr}'"
                    )
                })
            })
            .transpose()?;

        let mut providers = Vec::with_capacity(1 + additional.len());
        for name_or_path in std::iter::once(target).chain(additional.iter().map(String::as_str)) {
            let resolved = resolve_provider(name_or_path)?;
            if providers
                .iter()
                .any(|hosted: &HostedProvider| hosted.plane == resolved.plane)
            {
                anyhow::bail!(
                    "provider '{}' is named more than once",
                    resolved.plane.provider_name()
                );
            }
            providers.push(resolved);
        }

        Ok(Self {
            data_dir,
            providers,
            carrier_addr,
        })
    }

    pub(crate) fn provider_names(&self) -> Vec<&'static str> {
        self.providers
            .iter()
            .map(|hosted| hosted.plane.provider_name())
            .collect()
    }

    fn ready_receipt_path(&self) -> PathBuf {
        self.data_dir.join(READY_RECEIPT_RELATIVE_PATH)
    }
}

/// Interpret a `--with` / target argument as a binary path when it looks like
/// one, otherwise as an installed provider name.
fn resolve_provider(name_or_path: &str) -> anyhow::Result<HostedProvider> {
    let name = provider_name_of(name_or_path);
    let plane = ProviderPlane::from_provider_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "'{name_or_path}' is not a hostable native provider; supported providers are: {}",
            ProviderPlane::supported_set()
        )
    })?;

    let binary_path = if looks_like_path(name_or_path) {
        let path = PathBuf::from(name_or_path);
        if !path.is_file() {
            anyhow::bail!("provider binary '{}' is not a file", path.display());
        }
        path
    } else {
        crate::find_installed_provider_binary(name).ok_or_else(|| {
            anyhow::anyhow!("{name} is not installed.\n  Run:\n    elastos setup --with {name}")
        })?
    };

    Ok(HostedProvider { plane, binary_path })
}

fn looks_like_path(value: &str) -> bool {
    value.contains(std::path::MAIN_SEPARATOR)
}

/// The provider name an argument refers to: a path's file name, or the
/// argument itself.
fn provider_name_of(name_or_path: &str) -> &str {
    if looks_like_path(name_or_path) {
        Path::new(name_or_path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(name_or_path)
    } else {
        name_or_path
    }
}

/// A composed, running provider host. Holding one means every plane is
/// registered and the readiness receipt is published.
pub(crate) struct ProviderHost {
    /// Owning handle for every registered plane. Holding it is what keeps the
    /// provider bridges — and their child processes — alive for the host's
    /// lifetime; dropping it reaps them.
    _registry: Arc<ProviderRegistry>,
    carrier: elastos_server::carrier::CarrierRuntimeService,
    receipt_path: PathBuf,
    _host_lock: elastos_server::host_lock::HostProcessGuard,
}

impl ProviderHost {
    #[cfg(test)]
    pub(crate) fn registry(&self) -> &Arc<ProviderRegistry> {
        &self._registry
    }

    pub(crate) fn receipt_path(&self) -> &Path {
        &self.receipt_path
    }

    /// Retract readiness, then settle the Carrier node. The host lock is
    /// released when the guard drops with `self`.
    pub(crate) async fn shutdown(mut self) -> anyhow::Result<()> {
        if let Err(err) = fs::remove_file(&self.receipt_path) {
            if err.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    receipt = %self.receipt_path.display(),
                    "failed to remove provider host readiness receipt: {err}"
                );
            }
        }
        self.carrier.shutdown().await?;
        Ok(())
    }
}

/// Bring up the provider host's core planes and every planned provider.
///
/// Order matters: prerequisites are checked before anything is spawned, the
/// providers are registered before the Carrier node starts serving them, and
/// the readiness receipt is published only once every registration succeeded.
pub(crate) async fn compose(plan: &ProviderHostPlan) -> anyhow::Result<ProviderHost> {
    for hosted in &plan.providers {
        hosted.plane.check_prerequisites(&plan.data_dir)?;
    }

    let carrier_addr_label = plan
        .carrier_addr
        .map(|addr| addr.to_string())
        .unwrap_or_else(|| elastos_server::carrier::DEFAULT_CARRIER_BIND_ADDR.to_string());
    let host_lock = elastos_server::host_lock::acquire_host_process_lock(
        &plan.data_dir,
        "provider-host",
        &carrier_addr_label,
    )?;

    let device_key = elastos_identity::load_or_create_device_key(&plan.data_dir)
        .context("provider host identity is unavailable")?;
    let (signing_key, did) = elastos_identity::derive_did(&device_key);

    let registry = Arc::new(ProviderRegistry::new());
    server_infra::register_content_plane(&registry, &plan.data_dir).await?;
    tracing::info!(target_name = "content", "provider host plane registered");

    for hosted in &plan.providers {
        hosted
            .plane
            .register(&registry, &hosted.binary_path, &plan.data_dir)
            .await?;
        tracing::info!(
            provider = hosted.plane.provider_name(),
            target_name = hosted.plane.registry_target(),
            "provider host plane registered"
        );
    }

    let carrier_node = server_infra::start_carrier_plane(
        &registry,
        &signing_key,
        &did,
        &plan.data_dir,
        plan.carrier_addr,
    )
    .await
    .context("provider host Carrier plane failed to start")?;
    let carrier_bound = carrier_node
        .endpoint
        .bound_sockets()
        .first()
        .copied()
        .ok_or_else(|| {
            anyhow::anyhow!("provider host Carrier endpoint reported no bound socket")
        })?;
    tracing::info!(
        target_name = "carrier",
        did = %did,
        bound = %carrier_bound,
        "provider host plane registered"
    );

    let receipt_path = plan.ready_receipt_path();
    write_ready_receipt(
        &receipt_path,
        &did,
        &carrier_bound.to_string(),
        &plan.provider_names(),
    )?;

    Ok(ProviderHost {
        _registry: registry,
        carrier: elastos_server::carrier::CarrierRuntimeService::new(carrier_node),
        receipt_path,
        _host_lock: host_lock,
    })
}

/// Host the planned providers until SIGTERM or SIGINT, then retract readiness.
pub(crate) async fn run(plan: ProviderHostPlan) -> anyhow::Result<()> {
    let host = compose(&plan).await?;
    tracing::info!(
        providers = %plan.provider_names().join(","),
        receipt = %host.receipt_path().display(),
        "provider host ready"
    );

    wait_for_shutdown_signal().await?;
    tracing::info!("provider host shutting down");
    host.shutdown().await
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> anyhow::Result<()> {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("failed to install SIGTERM handler")?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result.context("failed to await SIGINT")?,
        _ = sigterm.recv() => {}
    }
    Ok(())
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() -> anyhow::Result<()> {
    tokio::signal::ctrl_c()
        .await
        .context("failed to await SIGINT")
}

/// Publish the owner-only readiness receipt peers' operators poll for.
fn write_ready_receipt(
    path: &Path,
    did: &str,
    carrier_bound: &str,
    providers: &[&'static str],
) -> anyhow::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        anyhow::anyhow!("provider host readiness receipt has no parent directory")
    })?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    #[cfg(unix)]
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to restrict {}", parent.display()))?;

    let receipt = serde_json::json!({
        "did": did,
        "carrier_bound": carrier_bound,
        "providers": providers,
        "started_at": elastos_server::auth::now_ts(),
    });
    let bytes = serde_json::to_vec_pretty(&receipt)
        .context("failed to encode provider host readiness receipt")?;

    // A crashed predecessor can leave a stale receipt; truncate rather than
    // refuse, but never follow a symlink into someone else's file.
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    let mut file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    #[cfg(unix)]
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to restrict {}", path.display()))?;
    file.write_all(&bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_resolve_to_planes_and_registry_targets() {
        for plane in ProviderPlane::ALL {
            assert_eq!(
                ProviderPlane::from_provider_name(plane.provider_name()),
                Some(*plane)
            );
            assert!(!plane.registry_target().is_empty());
        }
        assert_eq!(ProviderPlane::from_provider_name("dkms-provider"), None);
    }

    #[test]
    fn a_path_argument_names_its_plane_by_file_name() {
        assert_eq!(
            provider_name_of("/opt/elastos/bin/custody-provider"),
            "custody-provider"
        );
        assert_eq!(provider_name_of("ipfs-provider"), "ipfs-provider");
    }

    #[test]
    fn a_missing_provider_binary_path_fails_closed() {
        let error = resolve_provider("/nonexistent/dir/ipfs-provider")
            .expect_err("a missing binary must fail closed");
        assert!(error.to_string().contains("is not a file"), "{error}");
    }

    #[test]
    fn the_same_provider_cannot_be_hosted_twice() {
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("ipfs-provider");
        std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
        let binary = binary.to_string_lossy().into_owned();
        let error = ProviderHostPlan::resolve(
            temp.path().to_path_buf(),
            &binary,
            std::slice::from_ref(&binary),
            None,
        )
        .expect_err("a repeated provider must fail closed");
        assert!(error.to_string().contains("more than once"), "{error}");
    }

    #[test]
    fn a_malformed_carrier_addr_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let error = ProviderHostPlan::resolve(
            temp.path().to_path_buf(),
            "ipfs-provider",
            &[],
            Some("not-an-address"),
        )
        .expect_err("a malformed --carrier-addr must fail closed");
        assert!(error.to_string().contains("--carrier-addr"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn the_ready_receipt_is_owner_only_and_overwrites_a_stale_one() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(READY_RECEIPT_RELATIVE_PATH);
        write_ready_receipt(&path, "did:key:zStale", "0.0.0.0:4433", &["ipfs-provider"]).unwrap();
        write_ready_receipt(
            &path,
            "did:key:zFresh",
            "0.0.0.0:4433",
            &["custody-provider", "ipfs-provider"],
        )
        .unwrap();

        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(receipt["did"], "did:key:zFresh");
        assert_eq!(
            receipt["providers"],
            serde_json::json!(["custody-provider", "ipfs-provider"])
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }
}
