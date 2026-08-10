//! Runtime-owned startup boundary for one configured collaboration network.

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use base64::Engine as _;
use elastos_runtime::provider::Provider;
use elastos_runtime::signature::SigningKey;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::collaboration_carrier::join_collaboration_network;
use crate::collaboration_core::CollaborationCore;
use crate::collaboration_default_conversation::MAX_DEFAULT_CONVERSATION_GRANT_BYTES;
use crate::collaboration_discovery_runtime::CollaborationDiscoveryService;
use crate::collaboration_network::{MAX_PROFILE_BYTES, MAX_TRUSTED_SIGNERS};
use crate::collaboration_presence::CollaborationPresenceProductPort;
use crate::collaboration_product::{CollaborationChatProductPort, CHAT_ROOM_CAPSULE};
use crate::collaboration_profile_loader::{
    validate_collaboration_network_configuration, CollaborationNetworkConfiguration,
    CollaborationProfileChainLoader, ValidatedCollaborationNetworkConfiguration,
    MAX_CHAIN_PROFILES,
};
use crate::collaboration_transport::CollaborationTransportDriver;

pub const COLLABORATION_STARTUP_CONFIG_FILE: &str = "collaboration-network-v1.json";
pub const COLLABORATION_STARTUP_CONFIG_SCHEMA: &str =
    "elastos.collaboration-network.startup-config/v1";

pub(crate) const MAX_STARTUP_CONFIG_BYTES: usize = 3 * 1024 * 1024;
const COLLABORATION_WORKER_CADENCE: Duration = Duration::from_secs(5);
const COLLABORATION_WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_OWNED_PRESENCE_CADENCE: Duration = Duration::from_secs(15);

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollaborationStartupConfigFile {
    pub(crate) schema: String,
    pub(crate) expected_network_id: String,
    pub(crate) trusted_profile_signer_dids: Vec<String>,
    pub(crate) profile_chain_base64: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) default_conversation_grant_base64: Option<String>,
}

pub(crate) struct ValidatedCollaborationStartupConfiguration {
    validated: ValidatedCollaborationNetworkConfiguration,
}

impl ValidatedCollaborationStartupConfiguration {
    pub(crate) fn network(&self) -> &ValidatedCollaborationNetworkConfiguration {
        &self.validated
    }
}

/// Opaque validated startup configuration. Callers cannot alter its verified
/// profile, grant, topology, or trust roots between loading and construction.
pub struct CollaborationStartupConfiguration {
    configuration: CollaborationNetworkConfiguration,
}

/// Owns the one collaboration worker started for this Runtime process.
pub struct CollaborationRuntimeService {
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
    discovery_task: Option<tokio::task::JoinHandle<()>>,
    presence_task: Option<tokio::task::JoinHandle<()>>,
    product_port: Option<CollaborationChatProductPort>,
    presence_port: Option<CollaborationPresenceProductPort>,
    discovery_service: Option<CollaborationDiscoveryService>,
    #[cfg(test)]
    worker_product_port: Option<CollaborationChatProductPort>,
    #[cfg(test)]
    worker_presence_port: Option<CollaborationPresenceProductPort>,
}

/// Load and durably accept the sole collaboration configuration from the
/// Runtime data root. A valid configured chain may update the accepted-head
/// marker before this function returns.
///
/// Absence means isolation and delegates to the accepted-head loader so removal
/// after configuration still fails closed. This function never creates a key,
/// Carrier node, collaboration core, subscription, or worker.
pub fn load_and_accept_collaboration_startup_configuration(
    data_root: &Path,
) -> anyhow::Result<CollaborationStartupConfiguration> {
    let loader = CollaborationProfileChainLoader::new(data_root);
    let Some(config_bytes) = read_startup_config_bytes(data_root)? else {
        return Ok(CollaborationStartupConfiguration {
            configuration: loader.load_absent()?,
        });
    };
    let validated = parse_and_validate_collaboration_startup_configuration(&config_bytes)?;
    let configuration = loader.accept_validated(validated.validated)?;
    Ok(CollaborationStartupConfiguration { configuration })
}

/// Parse and validate a startup configuration without I/O or accepted-head mutation.
pub(crate) fn parse_and_validate_collaboration_startup_configuration(
    config_bytes: &[u8],
) -> anyhow::Result<ValidatedCollaborationStartupConfiguration> {
    if config_bytes.is_empty() || config_bytes.len() > MAX_STARTUP_CONFIG_BYTES {
        anyhow::bail!("collaboration startup configuration has an invalid byte length");
    }
    let config: CollaborationStartupConfigFile = serde_json::from_slice(config_bytes)
        .context("invalid collaboration startup configuration")?;
    if canonical_startup_config_bytes(&config)? != config_bytes {
        anyhow::bail!("collaboration startup configuration is not canonical JSON");
    }
    if config.schema != COLLABORATION_STARTUP_CONFIG_SCHEMA {
        anyhow::bail!("unsupported collaboration startup configuration schema");
    }
    if config.trusted_profile_signer_dids.is_empty()
        || config.trusted_profile_signer_dids.len() > MAX_TRUSTED_SIGNERS
    {
        anyhow::bail!("collaboration startup signer set has an invalid size");
    }
    if config.profile_chain_base64.is_empty()
        || config.profile_chain_base64.len() > MAX_CHAIN_PROFILES
    {
        anyhow::bail!("collaboration startup profile chain has an invalid entry count");
    }
    let profile_chain = config
        .profile_chain_base64
        .iter()
        .map(|encoded| {
            decode_canonical_base64(encoded, MAX_PROFILE_BYTES, "signed profile envelope")
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let grant_bytes = config
        .default_conversation_grant_base64
        .as_deref()
        .map(|encoded| {
            decode_canonical_base64(
                encoded,
                MAX_DEFAULT_CONVERSATION_GRANT_BYTES,
                "default-conversation grant",
            )
        })
        .transpose()?;
    let validated = validate_collaboration_network_configuration(
        &config.expected_network_id,
        &config.trusted_profile_signer_dids,
        &profile_chain,
        grant_bytes.as_deref(),
    )?;
    Ok(ValidatedCollaborationStartupConfiguration { validated })
}

/// Start the configured default-conversation service with the Runtime's
/// existing device-derived key and exact built-in Carrier gossip provider.
pub async fn start_collaboration_runtime_service(
    data_root: &Path,
    signing_key: SigningKey,
    configuration: CollaborationStartupConfiguration,
    carrier: Option<Arc<dyn Provider>>,
    provider_registry: Arc<elastos_runtime::provider::ProviderRegistry>,
) -> anyhow::Result<Option<CollaborationRuntimeService>> {
    let CollaborationNetworkConfiguration::Configured {
        profile,
        grant: Some(grant),
    } = configuration.configuration
    else {
        return Ok(None);
    };
    let discovery_signing_key = SigningKey::from_bytes(&signing_key.to_bytes());
    let carrier = carrier.context(
        "configured default collaboration conversation requires the built-in Carrier provider",
    )?;
    let core = Arc::new(CollaborationCore::new(
        data_root,
        signing_key,
        (*profile).clone(),
        grant,
        CHAT_ROOM_CAPSULE,
    )?);
    let discovery_service = CollaborationDiscoveryService::new(
        discovery_signing_key,
        (*profile).clone(),
        provider_registry,
    )
    .await?;
    let product_port = CollaborationChatProductPort::new(core.clone())?;
    let presence_port = CollaborationPresenceProductPort::new(core.clone())?;
    let registered = discovery_service.register_runtime_owned_contexts(data_root)?;
    if registered > 0 {
        tracing::info!("collaboration ready for {registered} Profile(s)");
    }
    let joined = join_collaboration_network(carrier, &profile).await?;
    let driver = CollaborationTransportDriver::new(core, joined);
    let (shutdown, shutdown_rx) = watch::channel(false);
    let discovery_shutdown_rx = shutdown_rx.clone();
    let presence_shutdown_rx = shutdown_rx.clone();
    let task = tokio::spawn(run_collaboration_worker(
        driver,
        product_port.clone(),
        presence_port.clone(),
        data_root.to_path_buf(),
        shutdown_rx,
    ));
    let discovery_task = tokio::spawn(run_collaboration_discovery_sync_worker(
        discovery_service.clone(),
        discovery_shutdown_rx,
    ));
    let presence_task = tokio::spawn(run_runtime_owned_presence_worker(
        data_root.to_path_buf(),
        presence_port.clone(),
        discovery_service.clone(),
        presence_shutdown_rx,
    ));
    Ok(Some(CollaborationRuntimeService {
        shutdown,
        task: Some(task),
        discovery_task: Some(discovery_task),
        presence_task: Some(presence_task),
        product_port: Some(product_port.clone()),
        presence_port: Some(presence_port.clone()),
        discovery_service: Some(discovery_service),
        #[cfg(test)]
        worker_product_port: Some(product_port),
        #[cfg(test)]
        worker_presence_port: Some(presence_port),
    }))
}

impl CollaborationRuntimeService {
    /// Return the opaque Chat product port retained by this configured service.
    pub fn chat_product_port(&self) -> CollaborationChatProductPort {
        self.product_port
            .clone()
            .expect("a configured collaboration service always owns its product port")
    }

    /// Return the opaque presence port retained by this configured service.
    pub fn presence_product_port(&self) -> CollaborationPresenceProductPort {
        self.presence_port
            .clone()
            .expect("a configured collaboration service always owns its presence port")
    }

    pub fn discovery_service(&self) -> CollaborationDiscoveryService {
        self.discovery_service
            .clone()
            .expect("a configured collaboration service always owns its discovery service")
    }

    pub fn gateway_context(&self) -> crate::api::gateway::GatewayCollaborationContext {
        crate::api::gateway::GatewayCollaborationContext {
            chat_product_port: Some(self.chat_product_port()),
            presence_product_port: Some(self.presence_product_port()),
            discovery_service: Some(self.discovery_service()),
        }
    }

    #[cfg(test)]
    fn test_worker_shares_product_core(&self) -> bool {
        let port = self.product_port.as_ref().unwrap();
        let worker = self.worker_product_port.as_ref().unwrap();
        let presence = self.presence_port.as_ref().unwrap();
        let worker_presence = self.worker_presence_port.as_ref().unwrap();
        port.test_shares_core_with(worker)
            && presence.test_shares_core_with(port.test_core())
            && worker_presence.test_shares_core_with(port.test_core())
    }

    /// Cancel and join the exact worker before its Carrier handle is released.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(true);
        let chat_result = join_collaboration_task(self.task.take(), "collaboration worker").await;
        let discovery_result =
            join_collaboration_task(self.discovery_task.take(), "collaboration discovery worker")
                .await;
        let presence_result =
            join_collaboration_task(self.presence_task.take(), "collaboration presence worker")
                .await;
        chat_result.and(discovery_result).and(presence_result)
    }

    #[cfg(test)]
    pub(crate) fn test_pending_service(stopped: Arc<std::sync::atomic::AtomicBool>) -> Self {
        let (shutdown, mut shutdown_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            let _ = shutdown_rx.changed().await;
            stopped.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        Self {
            shutdown,
            task: Some(task),
            discovery_task: None,
            presence_task: None,
            product_port: None,
            presence_port: None,
            discovery_service: None,
            worker_product_port: None,
            worker_presence_port: None,
        }
    }
}

impl Drop for CollaborationRuntimeService {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = &self.task {
            task.abort();
        }
        if let Some(task) = &self.discovery_task {
            task.abort();
        }
        if let Some(task) = &self.presence_task {
            task.abort();
        }
    }
}

async fn join_collaboration_task(
    task: Option<tokio::task::JoinHandle<()>>,
    name: &str,
) -> anyhow::Result<()> {
    let Some(mut task) = task else {
        return Ok(());
    };
    match tokio::time::timeout(COLLABORATION_WORKER_SHUTDOWN_TIMEOUT, &mut task).await {
        Ok(joined) => joined.with_context(|| format!("{name} failed during shutdown")),
        Err(_) => {
            task.abort();
            let _ = task.await;
            anyhow::bail!("{name} did not stop within its shutdown bound")
        }
    }
}

async fn run_collaboration_worker(
    driver: CollaborationTransportDriver,
    product_port: CollaborationChatProductPort,
    presence_port: CollaborationPresenceProductPort,
    data_root: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        let cycle = run_collaboration_worker_cycle_with_presence(
            &driver,
            &product_port,
            &presence_port,
            &data_root,
            now_secs(),
        );
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = cycle => {}
        }
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(COLLABORATION_WORKER_CADENCE) => {}
        }
    }
}

/// Discovery sync has the same Runtime lifecycle and shutdown signal as the
/// collaboration worker, while remaining isolated from Chat/presence Carrier
/// progress when a relay is slow or unavailable.
async fn run_collaboration_discovery_sync_worker(
    discovery_service: CollaborationDiscoveryService,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            return;
        }
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = discovery_service.sync_registered_contexts_once(now_secs()) => {}
        }
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = tokio::time::sleep(COLLABORATION_WORKER_CADENCE) => {}
        }
    }
}

async fn run_runtime_owned_presence_worker(
    data_root: PathBuf,
    presence_port: CollaborationPresenceProductPort,
    discovery_service: CollaborationDiscoveryService,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(
        RUNTIME_OWNED_PRESENCE_CADENCE.as_millis() as u64,
    ));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    return;
                }
            }
            _ = ticker.tick() => {}
        }
        if let Err(err) = crate::api::gateway::publish_runtime_owned_presence(
            &data_root,
            &presence_port,
            Some(&discovery_service),
            now_secs(),
        ) {
            tracing::debug!(error = %err, "runtime presence refresh failed");
        }
    }
}

async fn run_collaboration_worker_cycle_with_presence(
    driver: &CollaborationTransportDriver,
    product_port: &CollaborationChatProductPort,
    presence_port: &CollaborationPresenceProductPort,
    data_root: &Path,
    now: u64,
) {
    match product_port.pending_outgoing_messages(now) {
        Ok(messages) => {
            for message in &messages {
                if product_port
                    .project_prepared_message(data_root, message, None)
                    .is_err()
                {
                    tracing::warn!("collaboration Chat outgoing projection cycle failed");
                    break;
                }
            }
        }
        Err(_) => tracing::warn!("collaboration Chat outgoing projection cycle failed"),
    }
    match presence_port.pending_outgoing_presences(now) {
        Ok(presences) => {
            for presence in &presences {
                if presence_port
                    .project_prepared_presence(presence, now)
                    .is_err()
                {
                    tracing::warn!("collaboration presence outgoing projection cycle failed");
                    break;
                }
            }
        }
        Err(_) => tracing::warn!("collaboration presence outgoing projection cycle failed"),
    }
    if driver.retry_outgoing_once(now).await.is_err() {
        tracing::warn!("collaboration outgoing retry cycle failed");
    }
    if driver.process_incoming_once(now).await.is_err() {
        tracing::warn!("collaboration incoming cycle failed");
    }
    match product_port.pending_messages() {
        Ok(handoffs) => {
            for handoff in &handoffs {
                if product_port.project_handoff(data_root, handoff).is_err() {
                    tracing::warn!("collaboration Chat projection cycle failed");
                    break;
                }
            }
        }
        Err(_) => tracing::warn!("collaboration Chat projection cycle failed"),
    }
    match presence_port.pending_presences() {
        Ok(handoffs) => {
            for handoff in &handoffs {
                if presence_port.project_handoff(handoff, now).is_err() {
                    tracing::warn!("collaboration presence projection cycle failed");
                    break;
                }
            }
        }
        Err(_) => tracing::warn!("collaboration presence projection cycle failed"),
    }
}

#[cfg(test)]
async fn run_collaboration_worker_cycle(
    driver: &CollaborationTransportDriver,
    product_port: &CollaborationChatProductPort,
    data_root: &Path,
    now: u64,
) {
    let presence_port = CollaborationPresenceProductPort::new(product_port.test_core().clone())
        .expect("test Chat port owns a valid presence core");
    run_collaboration_worker_cycle_with_presence(
        driver,
        product_port,
        &presence_port,
        data_root,
        now,
    )
    .await;
}

fn read_startup_config_bytes(data_root: &Path) -> anyhow::Result<Option<Vec<u8>>> {
    let root_metadata = match fs::symlink_metadata(data_root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        anyhow::bail!("collaboration startup data root must be a real directory");
    }
    let path = startup_config_path(data_root);
    match fs::symlink_metadata(&path) {
        Ok(_) => read_collaboration_startup_config_candidate(&path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn read_collaboration_startup_config_candidate(path: &Path) -> anyhow::Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "failed to inspect collaboration startup configuration {}",
            path.display()
        )
    })?;
    validate_owner_only_regular_file(path, &metadata)?;
    let metadata_len = usize::try_from(metadata.len())
        .context("collaboration startup configuration length does not fit memory bounds")?;
    if metadata_len > MAX_STARTUP_CONFIG_BYTES {
        anyhow::bail!("collaboration startup configuration exceeds its byte limit");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    validate_owner_only_regular_file(path, &file.metadata()?)?;
    let read_limit = u64::try_from(MAX_STARTUP_CONFIG_BYTES)?
        .checked_add(1)
        .context("collaboration startup configuration read bound overflow")?;
    let mut bytes = Vec::with_capacity(metadata_len);
    file.take(read_limit).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STARTUP_CONFIG_BYTES {
        anyhow::bail!("collaboration startup configuration exceeds its byte limit");
    }
    Ok(bytes)
}

fn startup_config_path(data_root: &Path) -> PathBuf {
    data_root.join(COLLABORATION_STARTUP_CONFIG_FILE)
}

pub(crate) fn canonical_startup_config_bytes(
    config: &CollaborationStartupConfigFile,
) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(&serde_json::to_value(config)?)?)
}

fn decode_canonical_base64(
    encoded: &str,
    max_decoded_bytes: usize,
    field: &str,
) -> anyhow::Result<Vec<u8>> {
    let max_encoded_bytes = max_decoded_bytes.div_ceil(3) * 4;
    if encoded.is_empty() || encoded.len() > max_encoded_bytes {
        anyhow::bail!("collaboration {field} base64 has an invalid byte length");
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .with_context(|| format!("collaboration {field} is not valid base64"))?;
    if decoded.is_empty()
        || decoded.len() > max_decoded_bytes
        || base64::engine::general_purpose::STANDARD.encode(&decoded) != encoded
    {
        anyhow::bail!("collaboration {field} is not canonical bounded base64");
    }
    Ok(decoded)
}

fn validate_owner_only_regular_file(path: &Path, metadata: &fs::Metadata) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!(
            "collaboration startup configuration is not a regular file: {}",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            anyhow::bail!(
                "collaboration startup configuration is not owner-only: {}",
                path.display()
            );
        }
    }
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::VecDeque;
    use std::io::Write;
    use std::sync::Mutex;
    use std::time::Instant;

    use elastos_common::collaboration_protocol::collaboration_message_envelope_sha256;
    use elastos_runtime::provider::{
        ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use elastos_runtime::signature::{generate_keypair, SigningKey};
    use sha2::{Digest, Sha256};

    use crate::collaboration_carrier::join_collaboration_network;
    use crate::collaboration_core::{CollaborationCore, WriteFault};
    use crate::collaboration_default_conversation::{
        canonical_default_conversation_grant_bytes, DefaultConversationAdmissionPolicy,
        DefaultConversationGrant, DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
    };
    use crate::collaboration_device_authority::DefaultConversationDeviceAuthority;
    use crate::collaboration_network::{
        canonical_collaboration_network_profile_payload_bytes, CollaborationNetworkProfile,
        DefaultConversationGrantDescriptor, SignedCollaborationNetworkProfile,
        COLLABORATION_NETWORK_PROFILE_SCHEMA, COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
    };
    use crate::collaboration_presence::presence_request_binding;
    use crate::collaboration_product::chat_message_request_binding;
    use crate::collaboration_transport::CollaborationTransportDriver;
    use crate::esp_binding::esp_request_binding;

    const NETWORK: &str = "collaboration-startup-test";
    const CONVERSATION: &str = "default-conversation";
    const SERVICE: &str = "chat";
    const NOW: u64 = 1_800_000_000;
    const TTL: u64 = 300;

    fn profile_for_endpoint(
        endpoint_key: &SigningKey,
        display_name: &str,
    ) -> crate::collaboration_profile_authority::VerifiedCollaborationProfileDocument {
        let (profile_key, _) = generate_keypair();
        crate::collaboration_profile_authority::signed_profile_document_for_test(
            &profile_key,
            display_name,
            None,
            1,
            None,
            NOW,
            vec![crate::crypto::encode_did_key(&endpoint_key.verifying_key())],
        )
        .unwrap()
    }

    struct ConfigMaterial {
        signer_did: String,
        profile_bytes: Vec<u8>,
        grant_bytes: Option<Vec<u8>>,
    }

    enum FakeReply {
        JoinEcho,
        Value(serde_json::Value),
        Error(&'static str),
        Pending,
    }

    struct FakeCarrier {
        requests: Mutex<Vec<serde_json::Value>>,
        replies: Mutex<VecDeque<FakeReply>>,
    }

    impl FakeCarrier {
        fn new(replies: impl IntoIterator<Item = FakeReply>) -> Arc<Self> {
            Arc::new(Self {
                requests: Mutex::new(Vec::new()),
                replies: Mutex::new(replies.into_iter().collect()),
            })
        }

        fn push(&self, replies: impl IntoIterator<Item = FakeReply>) {
            self.replies.lock().unwrap().extend(replies);
        }

        fn requests(&self) -> Vec<serde_json::Value> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Provider for FakeCarrier {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider(
                "fake Carrier supports raw operations only".to_string(),
            ))
        }

        fn schemes(&self) -> Vec<&'static str> {
            Vec::new()
        }

        fn name(&self) -> &'static str {
            "fake-collaboration-startup-carrier"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            self.requests.lock().unwrap().push(request.clone());
            let reply = self.replies.lock().unwrap().pop_front();
            match reply {
                Some(FakeReply::JoinEcho) => Ok(serde_json::json!({
                    "status": "ok",
                    "data": {"topic": request["topic"]},
                })),
                Some(FakeReply::Value(value)) => Ok(value),
                Some(FakeReply::Error(message)) => {
                    Err(ProviderError::Provider(message.to_string()))
                }
                Some(FakeReply::Pending) => std::future::pending().await,
                None => Err(ProviderError::Provider(
                    "fake Carrier has no queued response".to_string(),
                )),
            }
        }
    }

    fn raw_sha256_cid(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let multihash = cid::multihash::Multihash::<64>::wrap(0x12, digest.as_slice()).unwrap();
        cid::Cid::new_v1(0x55, multihash).to_string()
    }

    fn config_material(with_grant: bool) -> ConfigMaterial {
        let grant_bytes = with_grant.then(|| {
            canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
                schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
                network_id: NETWORK.to_string(),
                conversation_id: CONVERSATION.to_string(),
                sender_service: SERVICE.to_string(),
                admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
            })
            .unwrap()
        });
        let (signer, _) = generate_keypair();
        let signer_did = crate::crypto::encode_did_key(&signer.verifying_key());
        let payload = CollaborationNetworkProfile {
            schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
            network_id: NETWORK.to_string(),
            revision: 1,
            previous_profile_sha256: None,
            signer_did: signer_did.clone(),
            bootstrap_peers: Vec::new(),
            default_conversation: grant_bytes.as_ref().map(|bytes| {
                DefaultConversationGrantDescriptor {
                    grant_cid: raw_sha256_cid(bytes),
                }
            }),
        };
        let payload_bytes =
            canonical_collaboration_network_profile_payload_bytes(&payload).unwrap();
        let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
            &signer,
            COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
            &payload_bytes,
        );
        let profile_bytes = serde_json::to_vec(
            &serde_json::to_value(SignedCollaborationNetworkProfile {
                payload,
                signature,
                signer_did: envelope_signer,
            })
            .unwrap(),
        )
        .unwrap();
        ConfigMaterial {
            signer_did,
            profile_bytes,
            grant_bytes,
        }
    }

    fn config_file(material: &ConfigMaterial) -> CollaborationStartupConfigFile {
        CollaborationStartupConfigFile {
            schema: COLLABORATION_STARTUP_CONFIG_SCHEMA.to_string(),
            expected_network_id: NETWORK.to_string(),
            trusted_profile_signer_dids: vec![material.signer_did.clone()],
            profile_chain_base64: vec![
                base64::engine::general_purpose::STANDARD.encode(&material.profile_bytes)
            ],
            default_conversation_grant_base64: material
                .grant_bytes
                .as_ref()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
        }
    }

    fn write_config(data_root: &Path, bytes: &[u8]) {
        let path = startup_config_path(data_root);
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .unwrap();
        }
    }

    fn configured_root(with_grant: bool) -> (tempfile::TempDir, ConfigMaterial) {
        let temp = tempfile::tempdir().unwrap();
        let material = config_material(with_grant);
        write_config(
            temp.path(),
            &canonical_startup_config_bytes(&config_file(&material)).unwrap(),
        );
        (temp, material)
    }

    fn send_remote() -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {"remote_peer_count": 1},
        }))
    }

    fn gossip_frame(
        authority: &DefaultConversationDeviceAuthority,
        envelope_bytes: &[u8],
    ) -> serde_json::Value {
        let frame = authority.prepare_transport_frame(envelope_bytes).unwrap();
        serde_json::json!({
            "content": String::from_utf8(frame).unwrap(),
        })
    }

    fn peek(cursor: u64, next_cursor: u64, messages: Vec<serde_json::Value>) -> FakeReply {
        let scanned = messages.len();
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "messages": messages,
                "scanned": scanned,
                "limit": 32,
                "cursor": cursor,
                "next_cursor": next_cursor,
            },
        }))
    }

    fn ack(cursor: u64, next_cursor: u64, advanced: bool) -> FakeReply {
        FakeReply::Value(serde_json::json!({
            "status": "ok",
            "data": {
                "cursor": cursor,
                "next_cursor": next_cursor,
                "advanced": advanced,
            },
        }))
    }

    fn request_ops(carrier: &FakeCarrier) -> Vec<String> {
        carrier
            .requests()
            .iter()
            .map(|request| request["op"].as_str().unwrap().to_string())
            .collect()
    }

    async fn wait_for_requests(carrier: &FakeCarrier, count: usize) {
        tokio::time::timeout(Duration::from_secs(1), async {
            while carrier.requests().len() < count {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[test]
    fn absent_config_is_pure_and_retained_marker_makes_removal_fail_closed() {
        let temp = tempfile::tempdir().unwrap();
        let before = fs::read_dir(temp.path()).unwrap().count();
        let isolated = load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        assert!(matches!(
            isolated.configuration,
            CollaborationNetworkConfiguration::Isolated
        ));
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), before);
        assert!(!temp.path().join("collaboration").exists());
        assert!(!temp.path().join("identity").exists());

        let material = config_material(false);
        write_config(
            temp.path(),
            &canonical_startup_config_bytes(&config_file(&material)).unwrap(),
        );
        let configured = load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        assert!(matches!(
            configured.configuration,
            CollaborationNetworkConfiguration::Configured { grant: None, .. }
        ));
        fs::remove_file(startup_config_path(temp.path())).unwrap();
        assert!(load_and_accept_collaboration_startup_configuration(temp.path()).is_err());
    }

    #[test]
    fn malformed_unsafe_or_untrusted_config_fails_before_any_runtime_effect() {
        fn rejected(bytes: Vec<u8>) {
            let temp = tempfile::tempdir().unwrap();
            write_config(temp.path(), &bytes);
            assert!(load_and_accept_collaboration_startup_configuration(temp.path()).is_err());
            assert!(!temp.path().join("collaboration").exists());
            assert!(!temp.path().join("identity").exists());
        }

        let material = config_material(true);
        let canonical = canonical_startup_config_bytes(&config_file(&material)).unwrap();
        let mut noncanonical = canonical.clone();
        noncanonical.push(b'\n');
        rejected(noncanonical);

        let mut unknown: serde_json::Value = serde_json::from_slice(&canonical).unwrap();
        unknown["unknown"] = serde_json::json!(true);
        rejected(serde_json::to_vec(&unknown).unwrap());

        let mut bad_base64 = config_file(&material);
        bad_base64.profile_chain_base64[0].push('=');
        rejected(canonical_startup_config_bytes(&bad_base64).unwrap());

        let mut bad_trust = config_file(&material);
        let (other_signer, _) = generate_keypair();
        bad_trust.trusted_profile_signer_dids =
            vec![crate::crypto::encode_did_key(&other_signer.verifying_key())];
        rejected(canonical_startup_config_bytes(&bad_trust).unwrap());

        let mut bad_profile = config_file(&material);
        let mut profile: serde_json::Value =
            serde_json::from_slice(&material.profile_bytes).unwrap();
        profile["signature"] = serde_json::json!("00".repeat(64));
        bad_profile.profile_chain_base64[0] =
            base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(&profile).unwrap());
        rejected(canonical_startup_config_bytes(&bad_profile).unwrap());

        let mut bad_grant = config_file(&material);
        bad_grant.default_conversation_grant_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(b"{}"));
        rejected(canonical_startup_config_bytes(&bad_grant).unwrap());

        let temp = tempfile::tempdir().unwrap();
        write_config(temp.path(), &canonical);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                startup_config_path(temp.path()),
                fs::Permissions::from_mode(0o644),
            )
            .unwrap();
            assert!(load_and_accept_collaboration_startup_configuration(temp.path()).is_err());
        }

        let oversized = vec![b' '; MAX_STARTUP_CONFIG_BYTES + 1];
        rejected(oversized);

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let temp = tempfile::tempdir().unwrap();
            let external = temp.path().join("external.json");
            fs::write(&external, canonical).unwrap();
            symlink(&external, startup_config_path(temp.path())).unwrap();
            assert!(load_and_accept_collaboration_startup_configuration(temp.path()).is_err());
        }
    }

    #[tokio::test]
    async fn configured_namespace_without_grant_starts_no_core_join_or_worker() {
        let isolated_temp = tempfile::tempdir().unwrap();
        let isolated =
            load_and_accept_collaboration_startup_configuration(isolated_temp.path()).unwrap();
        let (isolated_key, _) = generate_keypair();
        assert!(start_collaboration_runtime_service(
            isolated_temp.path(),
            isolated_key,
            isolated,
            None,
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap()
        .is_none());

        let (temp, _) = configured_root(false);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let (device_key, _) = generate_keypair();
        let service = start_collaboration_runtime_service(
            temp.path(),
            device_key,
            configuration,
            None,
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap();
        assert!(service.is_none());
        assert!(!temp
            .path()
            .join("collaboration/default-conversation")
            .exists());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn configured_service_worker_direct_gateway_and_supervisor_share_one_core() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let (device_key, _) = generate_keypair();
        let carrier = FakeCarrier::new([
            FakeReply::JoinEcho,
            send_remote(),
            peek(0, 0, Vec::new()),
            ack(0, 0, false),
        ]);
        let mut service = start_collaboration_runtime_service(
            temp.path(),
            device_key,
            configuration,
            Some(carrier.clone()),
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(service.test_worker_shares_product_core());

        let service_port = service.chat_product_port();
        let service_presence_port = service.presence_product_port();
        assert!(service_presence_port.test_shares_core_with(service_port.test_core()));
        let gateway_context = service.gateway_context();
        // Direct Gateway receives this exact service-owned clone.
        let gateway_state = crate::api::gateway::GatewayState {
            provider_registry: None,
            collaboration_chat_product_port: gateway_context.chat_product_port.clone(),
            collaboration_presence_product_port: gateway_context.presence_product_port.clone(),
            collaboration_discovery_service: gateway_context.discovery_service.clone(),
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: temp.path().join("gateway-cache"),
            data_dir: temp.path().to_path_buf(),
        };
        assert!(service_port.test_shares_core_with(
            gateway_state
                .collaboration_chat_product_port
                .as_ref()
                .unwrap()
        ));
        assert!(service_presence_port.test_shares_core_with(
            gateway_state
                .collaboration_presence_product_port
                .as_ref()
                .unwrap()
                .test_core()
        ));
        // run_serve and the public Gateway use this same Supervisor-held clone.
        let mut supervisor = crate::supervisor::Supervisor::new(
            temp.path().to_path_buf(),
            crate::setup::ComponentsManifest {
                external: std::collections::HashMap::new(),
                capsules: std::collections::HashMap::new(),
                profiles: std::collections::HashMap::new(),
            },
        );
        supervisor.set_collaboration_chat_product_port(service_port.clone());
        supervisor.set_collaboration_presence_product_port(service_presence_port.clone());
        assert!(service_port
            .test_shares_core_with(supervisor.test_collaboration_chat_product_port().unwrap()));
        assert!(service_presence_port.test_shares_core_with(
            supervisor
                .test_collaboration_presence_product_port()
                .unwrap()
                .test_core()
        ));

        let now = now_secs();
        let person_profile = service
            .chat_product_port()
            .test_person_profile("Local", None);
        let binding = chat_message_request_binding(
            "startup-product-request",
            "runtime-principal",
            "hello",
            &person_profile,
        )
        .unwrap();
        let first = service
            .chat_product_port()
            .prepare_message(binding.clone(), "hello", &person_profile, now)
            .unwrap();
        assert_eq!(
            service
                .chat_product_port()
                .prepare_message(binding, "hello", &person_profile, now)
                .unwrap(),
            first
        );

        wait_for_requests(&carrier, 4).await;
        service.shutdown().await.unwrap();
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_send",
                "gossip_peek",
                "gossip_ack"
            ]
        );
    }

    #[tokio::test]
    async fn one_worker_projects_presence_before_send_and_after_durable_receive() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let CollaborationNetworkConfiguration::Configured {
            profile,
            grant: Some(grant),
        } = configuration.configuration
        else {
            panic!("expected configured default conversation");
        };
        let (device_key, _) = generate_keypair();
        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key.clone(),
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let chat_port = CollaborationChatProductPort::new(core.clone()).unwrap();
        let presence_port = CollaborationPresenceProductPort::new(core.clone()).unwrap();
        let gateway_state = crate::api::gateway::GatewayState {
            provider_registry: None,
            collaboration_chat_product_port: Some(chat_port.clone()),
            collaboration_presence_product_port: Some(presence_port.clone()),
            collaboration_discovery_service: None,
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: temp.path().join("gateway-cache"),
            data_dir: temp.path().to_path_buf(),
        };
        let gateway_presence_port = gateway_state
            .collaboration_presence_product_port
            .as_ref()
            .unwrap();
        assert!(gateway_presence_port.test_shares_core_with(chat_port.test_core()));
        let person_profile = chat_port.test_person_profile("Local", None);
        let binding =
            presence_request_binding("local-presence", "runtime-principal", &person_profile)
                .unwrap();
        gateway_presence_port
            .prepare_presence(binding, &person_profile, NOW)
            .unwrap();

        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        carrier.push([send_remote(), peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle_with_presence(
            &driver,
            &chat_port,
            &presence_port,
            temp.path(),
            NOW + 1,
        )
        .await;
        assert_eq!(presence_port.snapshot(NOW + 1).unwrap().records().len(), 1);
        assert_eq!(
            request_ops(&carrier),
            [
                "gossip_join_exact",
                "gossip_send",
                "gossip_peek",
                "gossip_ack"
            ]
        );

        let (remote_key, _) = generate_keypair();
        let remote_profile = profile_for_endpoint(&remote_key, "Remote");
        let remote =
            DefaultConversationDeviceAuthority::new(remote_key, (*profile).clone(), grant.clone())
                .unwrap();
        let remote_presence = remote
            .prepare_profile_outgoing(
                &remote_profile,
                SERVICE,
                "elastos.chat.presence/v1",
                serde_json::json!({}),
                NOW + 1,
                45,
            )
            .unwrap();
        carrier.push([
            send_remote(),
            peek(
                0,
                1,
                vec![gossip_frame(&remote, remote_presence.envelope_bytes())],
            ),
            send_remote(),
            ack(0, 1, true),
        ]);
        run_collaboration_worker_cycle_with_presence(
            &driver,
            &chat_port,
            &presence_port,
            temp.path(),
            NOW + 2,
        )
        .await;
        assert_eq!(presence_port.snapshot(NOW + 2).unwrap().records().len(), 2);
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 0);

        let restarted_core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key,
                (*profile).clone(),
                grant,
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_presence =
            CollaborationPresenceProductPort::new(restarted_core.clone()).unwrap();
        assert_eq!(
            restarted_presence
                .snapshot(NOW + 2)
                .unwrap()
                .records()
                .len(),
            2
        );
        assert_eq!(
            restarted_core.summary().unwrap().pending_product_handoffs,
            0
        );
    }

    #[tokio::test]
    async fn worker_projects_known_chat_once_and_restart_keeps_it_acknowledged() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let CollaborationNetworkConfiguration::Configured {
            profile,
            grant: Some(grant),
        } = configuration.configuration
        else {
            panic!("expected configured default conversation");
        };
        let (device_key, _) = generate_keypair();
        let local_profile = profile_for_endpoint(&device_key, "Local");
        let local_did = local_profile.document().profile_did.clone();
        crate::room_service::seed_room_owner(
            temp.path(),
            &local_profile,
            crate::room_service::RoomOwnerSeedInput {
                title: "Chat".to_string(),
            },
        )
        .unwrap();
        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key.clone(),
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let product_port = CollaborationChatProductPort::new(core.clone()).unwrap();
        let (remote_key, _) = generate_keypair();
        let remote_profile = profile_for_endpoint(&remote_key, "Remote");
        let remote =
            DefaultConversationDeviceAuthority::new(remote_key, (*profile).clone(), grant.clone())
                .unwrap();
        let incoming = remote
            .prepare_profile_outgoing(
                &remote_profile,
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"projected once"}),
                NOW,
                TTL,
            )
            .unwrap();
        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        carrier.push([
            peek(0, 1, vec![gossip_frame(&remote, incoming.envelope_bytes())]),
            send_remote(),
            ack(0, 1, true),
        ]);

        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 0);
        let session = crate::room_service::start_local_runtime_session(
            temp.path(),
            &local_did,
            "Local runtime",
            "ElastOS shell",
        )
        .unwrap();
        assert_eq!(
            product_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );

        carrier.push([peek(1, 1, Vec::new()), ack(1, 1, false)]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 2).await;
        assert_eq!(
            product_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );

        let restarted_core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key,
                (*profile).clone(),
                grant,
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_port = CollaborationChatProductPort::new(restarted_core.clone()).unwrap();
        assert!(restarted_port.pending_messages().unwrap().is_empty());
        let restart_carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let restarted_joined = join_collaboration_network(restart_carrier.clone(), &profile)
            .await
            .unwrap();
        let restarted_driver =
            CollaborationTransportDriver::new(restarted_core.clone(), restarted_joined);
        restart_carrier.push([peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle(&restarted_driver, &restarted_port, temp.path(), NOW + 3)
            .await;
        assert_eq!(
            restarted_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );
        assert_eq!(
            restarted_core.summary().unwrap().pending_product_handoffs,
            0
        );
    }

    #[tokio::test]
    async fn worker_projects_outgoing_before_send_and_retries_room_and_core_failures() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let CollaborationNetworkConfiguration::Configured {
            profile,
            grant: Some(grant),
        } = configuration.configuration
        else {
            panic!("expected configured default conversation");
        };
        let (device_key, _) = generate_keypair();
        let local_profile = profile_for_endpoint(&device_key, "Local");
        let local_did = local_profile.document().profile_did.clone();
        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key.clone(),
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let product_port = CollaborationChatProductPort::new(core.clone()).unwrap();
        let person_profile = product_port.test_person_profile("Local", None);
        product_port
            .prepare_message(
                chat_message_request_binding(
                    "outgoing-projection",
                    "runtime-principal",
                    "durable outgoing",
                    &person_profile,
                )
                .unwrap(),
                "durable outgoing",
                &person_profile,
                NOW,
            )
            .unwrap();
        let room_root = elastos_common::localhost::rooted_localhost_fs_path(
            temp.path(),
            crate::room_service::room_root_uri(),
        )
        .unwrap();
        fs::create_dir_all(room_root.parent().unwrap()).unwrap();
        fs::write(&room_root, b"projection obstruction").unwrap();

        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        carrier.push([peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(
            core.pending_outgoing_product_projections(NOW + 1)
                .unwrap()
                .len(),
            1
        );
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());
        assert!(!request_ops(&carrier).iter().any(|op| op == "gossip_send"));

        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key.clone(),
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let product_port = CollaborationChatProductPort::new(core.clone()).unwrap();
        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        assert_eq!(
            core.pending_outgoing_product_projections(NOW + 1)
                .unwrap()
                .len(),
            1
        );
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());
        assert!(!request_ops(&carrier).iter().any(|op| op == "gossip_send"));

        fs::remove_file(&room_root).unwrap();
        let session = crate::room_service::start_local_runtime_session(
            temp.path(),
            &local_did,
            "Local runtime",
            "ElastOS shell",
        )
        .unwrap();
        core.inject_write_fault(WriteFault::BeforeWrite);
        carrier.push([peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(
            product_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );
        assert_eq!(
            core.pending_outgoing_product_projections(NOW + 1)
                .unwrap()
                .len(),
            1
        );
        assert!(!request_ops(&carrier).iter().any(|op| op == "gossip_send"));

        carrier.push([send_remote(), peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert!(core
            .pending_outgoing_product_projections(NOW + 1)
            .unwrap()
            .is_empty());
        assert_eq!(core.pending_outgoing(NOW + 1).unwrap().len(), 1);
        assert_eq!(
            request_ops(&carrier)
                .iter()
                .filter(|op| op.as_str() == "gossip_send")
                .count(),
            1
        );
        assert_eq!(
            product_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );

        let restarted_core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key,
                (*profile).clone(),
                grant,
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let restarted_port = CollaborationChatProductPort::new(restarted_core.clone()).unwrap();
        let restart_carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let restart_joined = join_collaboration_network(restart_carrier.clone(), &profile)
            .await
            .unwrap();
        let restart_driver =
            CollaborationTransportDriver::new(restarted_core.clone(), restart_joined);
        restart_carrier.push([send_remote(), peek(0, 0, Vec::new()), ack(0, 0, false)]);
        run_collaboration_worker_cycle(&restart_driver, &restarted_port, temp.path(), NOW + 2)
            .await;
        assert_eq!(
            restarted_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );
        assert_eq!(
            request_ops(&restart_carrier)
                .iter()
                .filter(|op| op.as_str() == "gossip_send")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn worker_projection_failure_stays_pending_then_retries_once() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let CollaborationNetworkConfiguration::Configured {
            profile,
            grant: Some(grant),
        } = configuration.configuration
        else {
            panic!("expected configured default conversation");
        };
        let (device_key, _) = generate_keypair();
        let local_profile = profile_for_endpoint(&device_key, "Local");
        let local_did = local_profile.document().profile_did.clone();
        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key,
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let product_port = CollaborationChatProductPort::new(core.clone()).unwrap();
        let (remote_key, _) = generate_keypair();
        let remote_profile = profile_for_endpoint(&remote_key, "Remote");
        let remote =
            DefaultConversationDeviceAuthority::new(remote_key, (*profile).clone(), grant).unwrap();
        let incoming = remote
            .prepare_profile_outgoing(
                &remote_profile,
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"body":"retry projection"}),
                NOW,
                TTL,
            )
            .unwrap();
        let room_root = elastos_common::localhost::rooted_localhost_fs_path(
            temp.path(),
            crate::room_service::room_root_uri(),
        )
        .unwrap();
        fs::create_dir_all(room_root.parent().unwrap()).unwrap();
        fs::write(&room_root, b"projection obstruction").unwrap();

        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        carrier.push([
            peek(0, 1, vec![gossip_frame(&remote, incoming.envelope_bytes())]),
            send_remote(),
            ack(0, 1, true),
        ]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);

        fs::remove_file(&room_root).unwrap();
        crate::room_service::seed_room_owner(
            temp.path(),
            &local_profile,
            crate::room_service::RoomOwnerSeedInput {
                title: "Chat".to_string(),
            },
        )
        .unwrap();
        carrier.push([peek(1, 1, Vec::new()), ack(1, 1, false)]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 2).await;
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 0);
        let session = crate::room_service::start_local_runtime_session(
            temp.path(),
            &local_did,
            "Local runtime",
            "ElastOS shell",
        )
        .unwrap();
        assert_eq!(
            product_port
                .conversation_poll(temp.path(), &session.token, 0)
                .unwrap()
                .objects
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn configured_default_room_requires_carrier_and_starts_one_joinable_worker() {
        let (missing_temp, _) = configured_root(true);
        let missing_configuration =
            load_and_accept_collaboration_startup_configuration(missing_temp.path()).unwrap();
        let (device_key, _) = generate_keypair();
        assert!(start_collaboration_runtime_service(
            missing_temp.path(),
            device_key,
            missing_configuration,
            None,
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .is_err());
        assert!(!missing_temp
            .path()
            .join("collaboration/default-conversation")
            .exists());

        let (temp, _) = configured_root(true);
        let marker = temp
            .path()
            .join("collaboration/config/accepted-profile-head-v1.json");
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let marker_before = fs::read(&marker).unwrap();
        let (device_key, _) = generate_keypair();
        let carrier = FakeCarrier::new([
            FakeReply::JoinEcho,
            peek(0, 0, Vec::new()),
            ack(0, 0, false),
        ]);
        let mut service = start_collaboration_runtime_service(
            temp.path(),
            device_key.clone(),
            configuration,
            Some(carrier.clone()),
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap()
        .unwrap();
        wait_for_requests(&carrier, 3).await;
        service.shutdown().await.unwrap();
        let requests = carrier.requests();
        assert_eq!(
            request_ops(&carrier),
            ["gossip_join_exact", "gossip_peek", "gossip_ack"]
        );
        assert_eq!(
            requests[1]["consumer_id"], requests[2]["consumer_id"],
            "one worker must retain one joined Carrier consumer"
        );
        let stopped_count = requests.len();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(carrier.requests().len(), stopped_count);
        assert!(!temp.path().join("identity").exists());

        let same_configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        assert_eq!(fs::read(&marker).unwrap(), marker_before);
        let restart_carrier = FakeCarrier::new([
            FakeReply::JoinEcho,
            peek(0, 0, Vec::new()),
            ack(0, 0, false),
        ]);
        let mut restarted = start_collaboration_runtime_service(
            temp.path(),
            device_key,
            same_configuration,
            Some(restart_carrier.clone()),
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap()
        .unwrap();
        wait_for_requests(&restart_carrier, 3).await;
        restarted.shutdown().await.unwrap();
        assert_eq!(
            request_ops(&restart_carrier),
            ["gossip_join_exact", "gossip_peek", "gossip_ack"]
        );
    }

    #[tokio::test]
    async fn worker_shutdown_cancels_and_joins_a_pending_provider_call() {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let (device_key, _) = generate_keypair();
        let carrier = FakeCarrier::new([FakeReply::JoinEcho, FakeReply::Pending]);
        let mut service = start_collaboration_runtime_service(
            temp.path(),
            device_key,
            configuration,
            Some(carrier.clone()),
            Arc::new(ProviderRegistry::new()),
        )
        .await
        .unwrap()
        .unwrap();
        wait_for_requests(&carrier, 2).await;
        let started = Instant::now();
        service.shutdown().await.unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        let stopped_count = carrier.requests().len();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(carrier.requests().len(), stopped_count);
    }

    #[tokio::test]
    async fn worker_cycle_attempts_directions_independently_and_retains_obligations_until_receipt()
    {
        let (temp, _) = configured_root(true);
        let configuration =
            load_and_accept_collaboration_startup_configuration(temp.path()).unwrap();
        let CollaborationNetworkConfiguration::Configured {
            profile,
            grant: Some(grant),
        } = configuration.configuration
        else {
            panic!("expected configured default conversation");
        };
        let (device_key, _) = generate_keypair();
        let local_profile = profile_for_endpoint(&device_key, "Local");
        let core = Arc::new(
            CollaborationCore::new(
                temp.path(),
                device_key,
                (*profile).clone(),
                grant.clone(),
                CHAT_ROOM_CAPSULE,
            )
            .unwrap(),
        );
        let payload = serde_json::json!({"text":"outgoing"});
        let payload_type = "elastos.chat.message/v1";
        let authenticated_payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                &local_profile,
                payload.clone(),
            )
            .unwrap();
        let intent = serde_json::json!({
            "payload_type": payload_type,
            "payload": authenticated_payload,
            "ttl_secs": TTL,
        });
        let outgoing = core
            .prepare_profile_outgoing(
                esp_request_binding(
                    "startup-worker-request",
                    "runtime-principal",
                    CHAT_ROOM_CAPSULE,
                    Some("elastos.chat.room"),
                    "message.send",
                    ["elastos://chat/message".to_string()],
                    &intent,
                ),
                &local_profile,
                payload_type,
                payload,
                NOW,
                TTL,
            )
            .unwrap();
        let unknown_payload = serde_json::json!({"text":"future outgoing"});
        let unknown_payload_type = "elastos.chat.message/v1";
        let authenticated_unknown_payload =
            crate::collaboration_default_conversation::profile_authenticated_conversation_payload(
                &local_profile,
                unknown_payload.clone(),
            )
            .unwrap();
        let unknown_intent = serde_json::json!({
            "payload_type": unknown_payload_type,
            "payload": authenticated_unknown_payload,
            "ttl_secs": TTL,
        });
        core.prepare_profile_outgoing(
            esp_request_binding(
                "startup-worker-future-request",
                "runtime-principal",
                CHAT_ROOM_CAPSULE,
                Some("elastos.chat.room"),
                "message.send",
                ["elastos://chat/message".to_string()],
                &unknown_intent,
            ),
            &local_profile,
            unknown_payload_type,
            unknown_payload,
            NOW,
            TTL,
        )
        .unwrap();
        let (remote_key, _) = generate_keypair();
        let remote_profile = profile_for_endpoint(&remote_key, "Remote");
        let remote_authority =
            DefaultConversationDeviceAuthority::new(remote_key, (*profile).clone(), grant).unwrap();
        let incoming = remote_authority
            .prepare_profile_outgoing(
                &remote_profile,
                SERVICE,
                "elastos.chat.message/v1",
                serde_json::json!({"text":"incoming"}),
                NOW,
                TTL,
            )
            .unwrap();
        let outgoing_authorized = remote_authority
            .authorize_incoming(
                outgoing.envelope_bytes(),
                serde_json::from_slice::<
                    elastos_common::collaboration_protocol::SignedCollaborationMessage,
                >(outgoing.envelope_bytes())
                .unwrap()
                .signer_did
                .as_str(),
                NOW + 1,
            )
            .unwrap();
        let remote_receipt = remote_authority
            .prepare_acceptance_receipt(&outgoing_authorized, NOW + 1)
            .unwrap();

        let carrier = FakeCarrier::new([FakeReply::JoinEcho]);
        let joined = join_collaboration_network(carrier.clone(), &profile)
            .await
            .unwrap();
        let driver = CollaborationTransportDriver::new(core.clone(), joined);
        let product_port = CollaborationChatProductPort::new(core.clone()).unwrap();

        carrier.push([FakeReply::Error("peek failed")]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());
        assert_eq!(
            core.pending_outgoing_product_projections(NOW + 1)
                .unwrap()
                .len(),
            2
        );
        assert!(product_port
            .pending_outgoing_messages(NOW + 1)
            .unwrap()
            .is_empty());
        assert_eq!(request_ops(&carrier), ["gossip_join_exact", "gossip_peek"]);

        core.inject_write_fault(WriteFault::BeforeWrite);
        carrier.push([peek(
            0,
            1,
            vec![gossip_frame(&remote_authority, incoming.envelope_bytes())],
        )]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 0);

        carrier.push([
            peek(
                0,
                1,
                vec![gossip_frame(&remote_authority, incoming.envelope_bytes())],
            ),
            send_remote(),
            FakeReply::Error("ack failed"),
        ]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());

        carrier.push([
            peek(
                0,
                1,
                vec![gossip_frame(&remote_authority, incoming.envelope_bytes())],
            ),
            send_remote(),
            ack(0, 1, true),
        ]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);

        carrier.push([
            peek(1, 2, vec![gossip_frame(&remote_authority, &remote_receipt)]),
            ack(1, 2, true),
        ]);
        run_collaboration_worker_cycle(&driver, &product_port, temp.path(), NOW + 1).await;
        assert!(core.pending_outgoing(NOW + 1).unwrap().is_empty());
        assert_eq!(core.summary().unwrap().remotely_accepted_outgoing, 0);
        assert_eq!(
            core.pending_outgoing_product_projections(NOW + 1)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(core.summary().unwrap().pending_product_handoffs, 1);
        assert!(product_port.pending_messages().unwrap().is_empty());
        assert_eq!(
            collaboration_message_envelope_sha256(outgoing.envelope_bytes()),
            outgoing.envelope_sha256()
        );
        assert!(!request_ops(&carrier).iter().any(|op| op == "gossip_recv"));
    }
}
