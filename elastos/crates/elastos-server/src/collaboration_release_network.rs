//! Release-pinned default collaboration network.
//!
//! The signed release names one collaboration network in `components.json`
//! and ships its startup configuration beside the manifest. Setup and update
//! install that configuration so a new Home joins the shared Community room
//! without an operator step. A Home stays isolated when its person chose
//! isolation before the first join.
//!
//! Trust comes from the release: the manifest pin fixes the exact bytes, the
//! network ID and the trusted profile-signer set. The configuration itself is
//! then checked by the same validator Runtime startup uses.

use std::fs;
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::collaboration_config::create_owner_only_file;
use crate::collaboration_startup::{
    parse_and_validate_collaboration_startup_configuration,
    read_collaboration_startup_config_candidate, CollaborationStartupConfigFile,
    COLLABORATION_STARTUP_CONFIG_FILE, MAX_STARTUP_CONFIG_BYTES,
};

/// The release copy of the pinned configuration, beside `components.json`.
pub const RELEASE_COLLABORATION_NETWORK_FILE: &str = "collaboration-network-release-v1.json";
/// Present when the person chose an isolated Home.
pub const COLLABORATION_ISOLATED_MARKER_FILE: &str = "collaboration-isolated-v1";
/// Serializes the isolation choice with Runtime startup's network acceptance.
const COLLABORATION_NETWORK_CHOICE_LOCK_FILE: &str = ".collaboration-network-choice.lock";
/// Source-home setup owns collaboration through its own explicit mode.
const SOURCE_HOME_COLLABORATION_MODE_ENV: &str = "ELASTOS_COLLABORATION_STARTUP_MODE";

/// The release pin for the default collaboration network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollaborationNetworkPin {
    /// Raw SHA-256 CIDv1 of the exact release configuration bytes.
    pub head_cid: String,
    pub expected_network_id: String,
    pub trusted_profile_signer_dids: Vec<String>,
}

/// What setup or update did with the release network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseNetworkOutcome {
    /// The release names no default network.
    NotPinned,
    /// Source-home setup manages collaboration explicitly.
    SourceHomeManaged,
    /// The person chose an isolated Home.
    Isolated,
    /// The Home now uses the release network.
    Joined { network_id: String },
    /// The Home already uses this exact release configuration.
    Unchanged { network_id: String },
    /// The Home moved forward along the same signed network chain.
    Advanced { network_id: String },
    /// The Home keeps a different configuration its operator installed.
    KeptExisting,
}

impl ReleaseNetworkOutcome {
    /// One line for setup and update output.
    pub fn summary_line(&self) -> Option<String> {
        match self {
            Self::NotPinned | Self::SourceHomeManaged => None,
            Self::Isolated => Some(
                "Community: this Home stays isolated, as chosen. It does not join the shared Community room."
                    .to_string(),
            ),
            Self::Joined { network_id }
            | Self::Unchanged { network_id }
            | Self::Advanced { network_id } => Some(format!(
                "Community: this Home joins the shared Community room ({network_id}). To keep a new Home isolated, run `elastos setup --isolated` before its first start."
            )),
            Self::KeptExisting => Some(
                "Community: this Home keeps its own collaboration network configuration.".to_string(),
            ),
        }
    }
}

/// Records that this Home stays isolated.
///
/// Before the first start this also removes the release network file that
/// setup installed, so Runtime never joins it. A Home whose Runtime already
/// accepted a network keeps it, because Runtime refuses to drop an accepted
/// network; leaving Community is a separate step. A configuration that is
/// not the release network belongs to the Home's operator and is kept.
pub fn choose_isolated(data_dir: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(data_dir)?;
    // Runtime startup takes the same lock to read and accept the network, so
    // the check, the choice and the removal below cannot interleave with it.
    let _choice = lock_network_choice(data_dir)?;
    let config = data_dir.join(COLLABORATION_STARTUP_CONFIG_FILE);
    let marker = data_dir.join(COLLABORATION_ISOLATED_MARKER_FILE);
    let release_config = match fs::symlink_metadata(&config) {
        Ok(_) => {
            require_not_started(data_dir)?;
            let bytes = read_collaboration_startup_config_candidate(&config)?;
            if !is_installed_release_network(data_dir, &bytes)? {
                anyhow::bail!(
                    "this Home has its own collaboration network configuration; isolation applies only to the release network"
                );
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("failed to inspect collaboration configuration"),
    };
    // The choice is recorded first. Startup honours it until the Home joins,
    // so an interrupted run stays isolated and a rerun finishes the removal.
    if fs::symlink_metadata(&marker).is_err() {
        create_owner_only_file(&marker, b"isolated\n", "collaboration isolation choice")?;
    }
    if release_config {
        fs::remove_file(&config).context("failed to remove the release collaboration network")?;
        fs::File::open(data_dir)?.sync_all()?;
        tracing::debug!("removed the release collaboration network before the first start");
    }
    Ok(())
}

/// Holds the network-choice lock for one isolation choice or one Runtime
/// startup acceptance. A missing data root has nothing to serialize.
pub(crate) fn lock_network_choice(
    data_dir: &Path,
) -> anyhow::Result<Option<crate::host_lock::FileLock>> {
    if !data_dir.is_dir() {
        return Ok(None);
    }
    crate::collaboration_core::lock_owner_only_file(
        &data_dir.join(COLLABORATION_NETWORK_CHOICE_LOCK_FILE),
    )
    .map(Some)
    .context("failed to lock the collaboration network choice")
}

/// True when Runtime startup must stay isolated: the person chose isolation
/// and Runtime has not accepted a network yet. A leftover release file from an
/// interrupted `--isolated` run is then ignored.
pub(crate) fn isolation_applies_at_startup(data_dir: &Path) -> bool {
    is_isolated(data_dir) && require_not_started(data_dir).is_ok()
}

/// Runtime creates the collaboration namespace when it first accepts a
/// network, so its presence means this Home already joined.
fn require_not_started(data_dir: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(data_dir.join("collaboration")) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("failed to inspect collaboration state"),
        Ok(_) => anyhow::bail!(
            "this Home already joined Community; isolation applies only before its first start"
        ),
    }
}

/// True when `bytes` are exactly the network the Home's installed release pins.
fn is_installed_release_network(data_dir: &Path, bytes: &[u8]) -> anyhow::Result<bool> {
    const MAX_INSTALLED_COMPONENTS_BYTES: u64 = 4 * 1024 * 1024;
    let path = data_dir.join("components.json");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("failed to inspect installed components"),
    };
    if !metadata.is_file() || metadata.len() > MAX_INSTALLED_COMPONENTS_BYTES {
        return Ok(false);
    }
    let manifest: crate::setup::ComponentsManifest =
        serde_json::from_slice(&fs::read(&path)?).context("installed components are invalid")?;
    let Some(pin) = manifest.collaboration_network else {
        return Ok(false);
    };
    Ok(crate::setup::catalog_head_cid(bytes)? == pin.head_cid)
}

pub fn is_isolated(data_dir: &Path) -> bool {
    fs::symlink_metadata(data_dir.join(COLLABORATION_ISOLATED_MARKER_FILE)).is_ok()
}

/// Checks release configuration bytes against the manifest pin.
pub fn verify_release_network_bytes(
    pin: &CollaborationNetworkPin,
    bytes: &[u8],
) -> anyhow::Result<()> {
    if bytes.is_empty() || bytes.len() > MAX_STARTUP_CONFIG_BYTES {
        anyhow::bail!("release collaboration network configuration has an invalid byte length");
    }
    let actual = crate::setup::catalog_head_cid(bytes)?;
    if actual != pin.head_cid {
        anyhow::bail!(
            "release collaboration network {actual} does not match the pinned raw SHA-256 CIDv1 {}",
            pin.head_cid
        );
    }
    parse_and_validate_collaboration_startup_configuration(bytes)
        .context("release collaboration network configuration is invalid")?;
    let config = parse_config(bytes)?;
    if config.expected_network_id != pin.expected_network_id {
        anyhow::bail!("release collaboration network ID does not match the release pin");
    }
    if sorted(&config.trusted_profile_signer_dids) != sorted(&pin.trusted_profile_signer_dids) {
        anyhow::bail!("release collaboration signer set does not match the release pin");
    }
    Ok(())
}

/// What a Home needs from the release network before any write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseNetworkPlan<'a> {
    /// The Home keeps its collaboration state and needs no release bytes.
    Skip(ReleaseNetworkOutcome),
    /// The Home needs the pinned release configuration.
    Install(&'a CollaborationNetworkPin),
}

/// Decides whether this Home takes the release network.
pub fn plan_release_network<'a>(
    data_dir: &Path,
    pin: Option<&'a CollaborationNetworkPin>,
) -> ReleaseNetworkPlan<'a> {
    let Some(pin) = pin else {
        return ReleaseNetworkPlan::Skip(ReleaseNetworkOutcome::NotPinned);
    };
    if std::env::var_os(SOURCE_HOME_COLLABORATION_MODE_ENV).is_some() {
        return ReleaseNetworkPlan::Skip(ReleaseNetworkOutcome::SourceHomeManaged);
    }
    if is_isolated(data_dir) {
        return ReleaseNetworkPlan::Skip(ReleaseNetworkOutcome::Isolated);
    }
    ReleaseNetworkPlan::Install(pin)
}

/// Fetches and verifies the release configuration this Home needs.
///
/// Signed setup and update call this with their other release downloads,
/// before they write anything. A Home that needs no release bytes fetches
/// nothing.
pub async fn fetch_release_network<F, Fut>(
    data_dir: &Path,
    pin: Option<&CollaborationNetworkPin>,
    fetch: F,
) -> anyhow::Result<Option<Vec<u8>>>
where
    F: FnOnce(&CollaborationNetworkPin) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<Vec<u8>>>,
{
    let ReleaseNetworkPlan::Install(pin) = plan_release_network(data_dir, pin) else {
        return Ok(None);
    };
    let bytes = fetch(pin)
        .await
        .context("could not fetch the release collaboration network configuration")?;
    verify_release_network_bytes(pin, &bytes)?;
    tracing::debug!(
        network_id = %pin.expected_network_id,
        bytes = bytes.len(),
        "verified release collaboration network"
    );
    Ok(Some(bytes))
}

/// Installs or advances the release network from bytes fetched earlier by
/// [`fetch_release_network`].
pub fn install_fetched_release_network(
    data_dir: &Path,
    pin: Option<&CollaborationNetworkPin>,
    fetched: Option<&[u8]>,
) -> anyhow::Result<ReleaseNetworkOutcome> {
    match plan_release_network(data_dir, pin) {
        ReleaseNetworkPlan::Skip(outcome) => Ok(outcome),
        ReleaseNetworkPlan::Install(pin) => {
            let release = fetched.context(
                "the release pins a collaboration network but its configuration was not fetched",
            )?;
            apply_release_network(data_dir, pin, release)
        }
    }
}

/// Installs or advances the release network from the release copy beside a
/// local `components.json`.
pub fn install_release_network(
    data_dir: &Path,
    pin: Option<&CollaborationNetworkPin>,
    release_copy: &Path,
) -> anyhow::Result<ReleaseNetworkOutcome> {
    let fetched = match plan_release_network(data_dir, pin) {
        ReleaseNetworkPlan::Install(_) => Some(fs::read(release_copy).with_context(|| {
            format!(
                "the release pins a collaboration network but {} is missing; use the complete signed release, or run `elastos setup --isolated` to keep this Home out of Community",
                release_copy.display()
            )
        })?),
        ReleaseNetworkPlan::Skip(_) => None,
    };
    install_fetched_release_network(data_dir, pin, fetched.as_deref())
}

fn apply_release_network(
    data_dir: &Path,
    pin: &CollaborationNetworkPin,
    release: &[u8],
) -> anyhow::Result<ReleaseNetworkOutcome> {
    verify_release_network_bytes(pin, release)?;
    let network_id = pin.expected_network_id.clone();
    let dest = data_dir.join(COLLABORATION_STARTUP_CONFIG_FILE);
    if fs::symlink_metadata(&dest).is_err() {
        fs::create_dir_all(data_dir)?;
        create_owner_only_file(&dest, release, "collaboration network configuration")?;
        return Ok(ReleaseNetworkOutcome::Joined { network_id });
    }
    let existing = read_collaboration_startup_config_candidate(&dest)?;
    if existing == release {
        return Ok(ReleaseNetworkOutcome::Unchanged { network_id });
    }
    if release_extends_existing(&existing, release)? {
        replace_owner_only_file(data_dir, &dest, release)?;
        return Ok(ReleaseNetworkOutcome::Advanced { network_id });
    }
    Ok(ReleaseNetworkOutcome::KeptExisting)
}

/// True when `release` is the same network and signer set with a signed
/// chain that strictly extends the installed one.
fn release_extends_existing(existing: &[u8], release: &[u8]) -> anyhow::Result<bool> {
    let Ok(existing) = parse_config(existing) else {
        return Ok(false);
    };
    let release = parse_config(release)?;
    Ok(existing.expected_network_id == release.expected_network_id
        && sorted(&existing.trusted_profile_signer_dids)
            == sorted(&release.trusted_profile_signer_dids)
        && release.profile_chain_base64.len() > existing.profile_chain_base64.len()
        && release
            .profile_chain_base64
            .starts_with(&existing.profile_chain_base64))
}

fn replace_owner_only_file(data_dir: &Path, dest: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let staged = data_dir.join(format!(".{COLLABORATION_STARTUP_CONFIG_FILE}.release.tmp"));
    let _ = fs::remove_file(&staged);
    create_owner_only_file(&staged, bytes, "collaboration network configuration update")?;
    fs::rename(&staged, dest).inspect_err(|_| {
        let _ = fs::remove_file(&staged);
    })?;
    fs::File::open(data_dir)?.sync_all()?;
    Ok(())
}

fn parse_config(bytes: &[u8]) -> anyhow::Result<CollaborationStartupConfigFile> {
    serde_json::from_slice(bytes).context("invalid collaboration network configuration")
}

fn sorted(values: &[String]) -> Vec<&str> {
    let mut values = values.iter().map(String::as_str).collect::<Vec<_>>();
    values.sort_unstable();
    values
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine as _;
    use elastos_runtime::signature::generate_keypair;
    use sha2::{Digest, Sha256};

    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes, CollaborationNetworkProfile,
        SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_startup::{
        canonical_startup_config_bytes, COLLABORATION_STARTUP_CONFIG_SCHEMA,
    };

    /// One signer's network, with the canonical startup configuration for
    /// every revision from 1 to `revisions`.
    pub(crate) fn signed_profile_chain_config(revisions: u64) -> (String, Vec<Vec<u8>>) {
        let (signer, _) = generate_keypair();
        let signer_did = crate::crypto::encode_signing_key_did(&signer);
        let network_id = format!(
            "release-network-{}",
            &hex::encode(Sha256::digest(signer_did.as_bytes()))[..12]
        );
        let mut envelopes: Vec<Vec<u8>> = Vec::new();
        let mut configs = Vec::new();
        for revision in 1..=revisions {
            let payload = CollaborationNetworkProfile {
                schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
                network_id: network_id.clone(),
                revision,
                previous_profile_sha256: envelopes
                    .last()
                    .map(|bytes| format!("sha256:{}", hex::encode(Sha256::digest(bytes)))),
                signer_did: signer_did.clone(),
                bootstrap_peers: Vec::new(),
                default_conversation: None,
            };
            let payload_bytes =
                canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
            let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
                &signer,
                COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
                &payload_bytes,
            );
            envelopes.push(
                serde_json::to_vec(
                    &serde_json::to_value(SignedCollaborationNetworkProfile {
                        payload,
                        signature,
                        signer_did: envelope_signer,
                    })
                    .unwrap(),
                )
                .unwrap(),
            );
            configs.push(
                canonical_startup_config_bytes(&CollaborationStartupConfigFile {
                    schema: COLLABORATION_STARTUP_CONFIG_SCHEMA.to_string(),
                    expected_network_id: network_id.clone(),
                    trusted_profile_signer_dids: vec![signer_did.clone()],
                    profile_chain_base64: envelopes
                        .iter()
                        .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes))
                        .collect(),
                    default_conversation_grant_base64: None,
                })
                .unwrap(),
            );
        }
        (signer_did, configs)
    }

    fn pin_for(bytes: &[u8]) -> CollaborationNetworkPin {
        let config = parse_config(bytes).unwrap();
        CollaborationNetworkPin {
            head_cid: crate::setup::catalog_head_cid(bytes).unwrap(),
            expected_network_id: config.expected_network_id,
            trusted_profile_signer_dids: config.trusted_profile_signer_dids,
        }
    }

    fn release_dir_with(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let release = tempfile::tempdir().unwrap();
        let path = release.path().join(RELEASE_COLLABORATION_NETWORK_FILE);
        fs::write(&path, bytes).unwrap();
        (release, path)
    }

    fn private_data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    #[test]
    fn a_new_home_joins_the_pinned_release_network() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let pin = pin_for(&chain[0]);

        let outcome = install_release_network(data.path(), Some(&pin), &release_path).unwrap();

        assert_eq!(
            outcome,
            ReleaseNetworkOutcome::Joined {
                network_id: pin.expected_network_id.clone()
            }
        );
        let dest = data.path().join(COLLABORATION_STARTUP_CONFIG_FILE);
        assert_eq!(fs::read(&dest).unwrap(), chain[0]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&dest).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(outcome
            .summary_line()
            .unwrap()
            .contains("elastos setup --isolated"));
        assert_eq!(
            install_release_network(data.path(), Some(&pin), &release_path).unwrap(),
            ReleaseNetworkOutcome::Unchanged {
                network_id: pin.expected_network_id
            },
            "setup again is idempotent"
        );
    }

    #[test]
    fn an_isolated_home_stays_isolated() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        choose_isolated(data.path()).unwrap();

        let outcome =
            install_release_network(data.path(), Some(&pin_for(&chain[0])), &release_path).unwrap();

        assert_eq!(outcome, ReleaseNetworkOutcome::Isolated);
        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
    }

    /// The Home's installed `components.json`, pinning `pin`.
    fn install_components_pin(data: &Path, pin: &CollaborationNetworkPin) {
        let manifest = serde_json::json!({
            "external": {},
            "profiles": {},
            "collaboration_network": pin,
        });
        fs::write(data.join("components.json"), manifest.to_string()).unwrap();
    }

    #[test]
    fn isolation_before_the_first_start_removes_the_release_network() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let pin = pin_for(&chain[0]);
        install_components_pin(data.path(), &pin);
        install_release_network(data.path(), Some(&pin), &release_path).unwrap();

        choose_isolated(data.path()).unwrap();

        assert!(is_isolated(data.path()));
        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
        // Setup and update now leave it uninstalled, and the choice is idempotent.
        assert_eq!(
            install_release_network(data.path(), Some(&pin), &release_path).unwrap(),
            ReleaseNetworkOutcome::Isolated
        );
        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
        choose_isolated(data.path()).unwrap();
    }

    #[test]
    fn isolation_is_refused_after_a_home_joined() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let pin = pin_for(&chain[0]);
        install_components_pin(data.path(), &pin);
        install_release_network(data.path(), Some(&pin), &release_path).unwrap();
        // Runtime accepted the network at its first start.
        fs::create_dir(data.path().join("collaboration")).unwrap();

        let err = choose_isolated(data.path()).unwrap_err().to_string();

        assert!(err.contains("already joined Community"), "{err}");
        assert!(!is_isolated(data.path()));
        assert_eq!(
            fs::read(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).unwrap(),
            chain[0]
        );
    }

    #[test]
    fn isolation_keeps_an_operator_network() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_other_signer, other) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        install_release_network(data.path(), Some(&pin_for(&chain[0])), &release_path).unwrap();

        // No installed pin, then a pin for another network: neither marks
        // this file as the release network.
        for pin in [None, Some(pin_for(&other[0]))] {
            if let Some(pin) = &pin {
                install_components_pin(data.path(), pin);
            }
            let err = choose_isolated(data.path()).unwrap_err().to_string();
            assert!(err.contains("its own collaboration network"), "{err}");
            assert!(!is_isolated(data.path()));
            assert_eq!(
                fs::read(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).unwrap(),
                chain[0]
            );
        }
    }

    #[test]
    fn an_interrupted_isolation_finishes_on_the_next_run() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let pin = pin_for(&chain[0]);
        install_components_pin(data.path(), &pin);
        install_release_network(data.path(), Some(&pin), &release_path).unwrap();
        // The choice was recorded, then the run stopped before the removal.
        create_owner_only_file(
            &data.path().join(COLLABORATION_ISOLATED_MARKER_FILE),
            b"isolated\n",
            "test",
        )
        .unwrap();

        // Until the rerun, startup honours the recorded choice: it stays
        // isolated and does not join the leftover release network.
        let startup =
            crate::collaboration_startup::load_and_accept_collaboration_startup_configuration(
                data.path(),
            )
            .unwrap();
        assert!(startup.is_isolated_for_test());
        assert!(fs::symlink_metadata(data.path().join("collaboration")).is_err());

        choose_isolated(data.path()).unwrap();

        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
    }

    #[test]
    fn isolation_waits_for_a_startup_that_is_accepting_the_network() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let pin = pin_for(&chain[0]);
        install_components_pin(data.path(), &pin);
        install_release_network(data.path(), Some(&pin), &release_path).unwrap();

        // A starting Runtime holds the lock while it reads and accepts.
        let startup = lock_network_choice(data.path()).unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let data_path = data.path().to_path_buf();
        let isolation = std::thread::spawn(move || {
            let result = choose_isolated(&data_path).map_err(|err| err.to_string());
            done_tx.send(()).unwrap();
            result
        });
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
        // The Runtime accepted the network before releasing the lock.
        fs::create_dir(data.path().join("collaboration")).unwrap();
        drop(startup);

        let err = isolation.join().unwrap().unwrap_err();
        assert!(err.contains("already joined Community"), "{err}");
        assert!(!is_isolated(data.path()));
        assert_eq!(
            fs::read(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).unwrap(),
            chain[0]
        );
    }

    #[test]
    fn release_bytes_must_match_the_pin() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_release, release_path) = release_dir_with(&chain[0]);
        let data = private_data_dir();
        let mut pin = pin_for(&chain[0]);
        pin.head_cid = crate::setup::catalog_head_cid(b"other").unwrap();

        let err = install_release_network(data.path(), Some(&pin), &release_path)
            .unwrap_err()
            .to_string();

        assert!(err.contains("does not match the pinned"), "{err}");
        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
    }

    #[test]
    fn release_network_id_and_signers_must_match_the_pin() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let mut pin = pin_for(&chain[0]);
        pin.expected_network_id = "another-network".to_string();
        assert!(verify_release_network_bytes(&pin, &chain[0])
            .unwrap_err()
            .to_string()
            .contains("network ID"));

        let mut pin = pin_for(&chain[0]);
        pin.trusted_profile_signer_dids = vec!["did:key:z6MkOther".to_string()];
        assert!(verify_release_network_bytes(&pin, &chain[0])
            .unwrap_err()
            .to_string()
            .contains("signer set"));
    }

    #[test]
    fn an_update_advances_along_the_same_signed_chain() {
        let (_signer, chain) = signed_profile_chain_config(2);
        let data = private_data_dir();
        let (_first, first_path) = release_dir_with(&chain[0]);
        install_release_network(data.path(), Some(&pin_for(&chain[0])), &first_path).unwrap();
        let (_second, second_path) = release_dir_with(&chain[1]);

        let outcome =
            install_release_network(data.path(), Some(&pin_for(&chain[1])), &second_path).unwrap();

        assert!(matches!(outcome, ReleaseNetworkOutcome::Advanced { .. }));
        let dest = data.path().join(COLLABORATION_STARTUP_CONFIG_FILE);
        assert_eq!(fs::read(&dest).unwrap(), chain[1]);
    }

    #[test]
    fn an_operator_configuration_for_another_network_is_kept() {
        let (_signer_a, operator_chain) = signed_profile_chain_config(1);
        let (_signer_b, release_chain) = signed_profile_chain_config(1);
        let data = private_data_dir();
        let dest = data.path().join(COLLABORATION_STARTUP_CONFIG_FILE);
        create_owner_only_file(&dest, &operator_chain[0], "operator configuration").unwrap();
        let (_release, release_path) = release_dir_with(&release_chain[0]);

        let outcome = install_release_network(
            data.path(),
            Some(&pin_for(&release_chain[0])),
            &release_path,
        )
        .unwrap();

        assert_eq!(outcome, ReleaseNetworkOutcome::KeptExisting);
        assert_eq!(fs::read(&dest).unwrap(), operator_chain[0]);
    }

    #[test]
    fn a_missing_release_copy_fails_when_the_release_pins_a_network() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let data = private_data_dir();
        let missing = data.path().join(RELEASE_COLLABORATION_NETWORK_FILE);

        let err = install_release_network(data.path(), Some(&pin_for(&chain[0])), &missing)
            .unwrap_err()
            .to_string();

        assert!(err.contains("is missing"), "{err}");
    }

    #[tokio::test]
    async fn a_home_that_needs_no_release_bytes_fetches_nothing() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let pin = pin_for(&chain[0]);
        let data = private_data_dir();
        let refuse = |_: &CollaborationNetworkPin| async {
            anyhow::bail!("this Home fetches no release network")
        };

        assert_eq!(
            fetch_release_network(data.path(), None, refuse)
                .await
                .unwrap(),
            None
        );
        choose_isolated(data.path()).unwrap();
        assert_eq!(
            fetch_release_network(data.path(), Some(&pin), refuse)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            install_fetched_release_network(data.path(), Some(&pin), None).unwrap(),
            ReleaseNetworkOutcome::Isolated
        );
    }

    #[tokio::test]
    async fn fetched_bytes_must_match_the_pin_before_install() {
        let (_signer, chain) = signed_profile_chain_config(1);
        let (_other, other) = signed_profile_chain_config(1);
        let pin = pin_for(&chain[0]);
        let data = private_data_dir();

        let fetched = fetch_release_network(data.path(), Some(&pin), |pin| {
            let head_cid = pin.head_cid.clone();
            let bytes = chain[0].clone();
            async move {
                assert_eq!(head_cid, crate::setup::catalog_head_cid(&bytes).unwrap());
                Ok(bytes)
            }
        })
        .await
        .unwrap();
        assert_eq!(fetched.as_deref(), Some(chain[0].as_slice()));

        let err = fetch_release_network(data.path(), Some(&pin), |_| {
            let bytes = other[0].clone();
            async move { Ok(bytes) }
        })
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("does not match the pinned"), "{err}");

        let err = install_fetched_release_network(data.path(), Some(&pin), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("was not fetched"), "{err}");
        assert!(fs::symlink_metadata(data.path().join(COLLABORATION_STARTUP_CONFIG_FILE)).is_err());
    }
}
