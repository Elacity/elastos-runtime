//! Offline operator provisioning for the canonical collaboration startup config.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine as _;
use clap::Subcommand;
use elastos_runtime::signature::SigningKey;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::collaboration_default_conversation::{
    canonical_default_conversation_grant_bytes, raw_sha256_cid, DefaultConversationAdmissionPolicy,
    DefaultConversationGrant, DEFAULT_CONVERSATION_GRANT_SCHEMA_V1,
};
use crate::collaboration_network::{
    canonical_collaboration_network_profile_payload_bytes, validate_collaboration_bootstrap_peer,
    CollaborationBootstrapPeer, CollaborationNetworkProfile, DefaultConversationGrantDescriptor,
    SignedCollaborationNetworkProfile, COLLABORATION_NETWORK_PROFILE_SCHEMA,
    COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
};
use crate::collaboration_product::CHAT_SERVICE;
use crate::collaboration_startup::{
    canonical_startup_config_bytes, parse_and_validate_collaboration_startup_configuration,
    read_collaboration_startup_config_candidate, CollaborationStartupConfigFile,
    COLLABORATION_STARTUP_CONFIG_SCHEMA,
};

const OPERATOR_RECEIPT_SCHEMA: &str = "elastos.collaboration-network.operator-receipt/v1";
const KEY_RECEIPT_SCHEMA: &str = "elastos.collaboration-network.authority-key-created/v1";
const LOCAL_BOOTSTRAP_RECEIPT_SCHEMA: &str =
    "elastos.collaboration-network.local-bootstrap-receipt/v1";
const MAX_BOOTSTRAP_RECEIPT_BYTES: usize = 16 * 1024;

#[derive(Debug, Subcommand)]
pub enum CollaborationConfigCommand {
    /// Create a new dedicated 32-byte collaboration configuration authority key.
    CreateAuthorityKey {
        #[arg(long)]
        key: PathBuf,
    },
    /// Generate a revision-1 startup configuration without network access.
    GenerateInitial {
        #[arg(long)]
        authority_key: PathBuf,
        #[arg(long)]
        network_id: String,
        #[arg(long)]
        conversation_id: String,
        #[arg(long)]
        bootstrap_peer: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Purely verify one candidate startup configuration.
    Verify {
        #[arg(long)]
        input: PathBuf,
    },
    /// Export this Runtime's canonical Carrier bootstrap peer receipt.
    ExportLocalBootstrapReceipt {
        #[arg(long)]
        data_root: PathBuf,
        #[arg(long, value_enum)]
        runtime_kind: crate::runtime_control::AttachableRuntimeKind,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Serialize)]
struct KeyCreatedReceipt {
    schema: &'static str,
    created: bool,
}

#[derive(Debug, Serialize)]
struct OperatorReceipt {
    schema: &'static str,
    network_id: String,
    conversation_id: String,
    signer_did: String,
    profile_sha256: String,
    grant_cid: String,
    bootstrap_node_id: String,
    config_sha256: String,
}

#[derive(Debug, Serialize)]
struct LocalBootstrapReceipt {
    schema: &'static str,
    node_id: String,
    output_sha256: String,
    created: bool,
}

pub async fn run_collaboration_config_command(
    command: CollaborationConfigCommand,
) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    run_collaboration_config_command_with_writer(command, &mut output).await
}

async fn run_collaboration_config_command_with_writer(
    command: CollaborationConfigCommand,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    match command {
        CollaborationConfigCommand::CreateAuthorityKey { key } => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes)
                .context("OS randomness unavailable for collaboration authority key")?;
            create_owner_only_file(&key, &bytes, "collaboration authority key")?;
            write_receipt(
                output,
                &KeyCreatedReceipt {
                    schema: KEY_RECEIPT_SCHEMA,
                    created: true,
                },
            )
        }
        CollaborationConfigCommand::GenerateInitial {
            authority_key,
            network_id,
            conversation_id,
            bootstrap_peer,
            output: output_path,
        } => {
            let signing_key = read_authority_key(&authority_key)?;
            let peer_bytes = read_owner_only_file(
                &bootstrap_peer,
                MAX_BOOTSTRAP_RECEIPT_BYTES,
                "collaboration bootstrap receipt",
            )?;
            let peer: CollaborationBootstrapPeer = serde_json::from_slice(&peer_bytes)
                .context("invalid collaboration bootstrap receipt")?;
            if serde_json::to_vec(&serde_json::to_value(&peer)?)? != peer_bytes {
                anyhow::bail!("collaboration bootstrap receipt is not canonical JSON");
            }
            let config_bytes =
                initial_config_bytes(&signing_key, &network_id, &conversation_id, peer)?;
            let receipt = verified_receipt(&config_bytes)?;
            create_owner_only_file(
                &output_path,
                &config_bytes,
                "collaboration startup configuration",
            )?;
            write_receipt(output, &receipt)
        }
        CollaborationConfigCommand::Verify { input } => {
            validate_owner_only_parent(&input)?;
            let bytes = read_collaboration_startup_config_candidate(&input)?;
            write_receipt(output, &verified_receipt(&bytes)?)
        }
        CollaborationConfigCommand::ExportLocalBootstrapReceipt {
            data_root,
            runtime_kind,
            output: output_path,
        } => {
            validate_private_output_target(&output_path)?;
            let (connect_ticket, node_id) =
                crate::operator_control::fetch_local_carrier_bootstrap(&data_root, runtime_kind)
                    .await?;
            write_local_bootstrap_receipt(connect_ticket, node_id, &output_path, output)
        }
    }
}

fn write_local_bootstrap_receipt(
    connect_ticket: String,
    node_id: String,
    output_path: &Path,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    validate_private_output_target(output_path)?;
    let peer = CollaborationBootstrapPeer {
        node_id,
        connect_ticket,
    };
    validate_collaboration_bootstrap_peer(&peer)?;
    let bytes = serde_json::to_vec(&serde_json::to_value(&peer)?)?;
    create_owner_only_file(output_path, &bytes, "collaboration bootstrap receipt")
        .map_err(|_| anyhow::anyhow!("failed to create collaboration bootstrap receipt"))?;
    write_receipt(
        output,
        &LocalBootstrapReceipt {
            schema: LOCAL_BOOTSTRAP_RECEIPT_SCHEMA,
            node_id: peer.node_id,
            output_sha256: sha256_label(&bytes),
            created: true,
        },
    )
}

fn initial_config_bytes(
    signing_key: &SigningKey,
    network_id: &str,
    conversation_id: &str,
    bootstrap_peer: CollaborationBootstrapPeer,
) -> anyhow::Result<Vec<u8>> {
    let signer_did = crate::crypto::encode_signing_key_did(&signing_key);
    let grant_bytes = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
        schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
        network_id: network_id.to_string(),
        conversation_id: conversation_id.to_string(),
        sender_service: CHAT_SERVICE.to_string(),
        admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
    })?;
    let grant_cid = raw_sha256_cid(&grant_bytes)?;
    let profile = CollaborationNetworkProfile {
        schema: COLLABORATION_NETWORK_PROFILE_SCHEMA.to_string(),
        network_id: network_id.to_string(),
        revision: 1,
        previous_profile_sha256: None,
        signer_did: signer_did.clone(),
        bootstrap_peers: vec![bootstrap_peer],
        default_conversation: Some(DefaultConversationGrantDescriptor { grant_cid }),
    };
    let payload_bytes = canonical_collaboration_network_profile_payload_bytes(&profile)?;
    let (signature, envelope_signer) = crate::crypto::domain_separated_sign(
        signing_key,
        COLLABORATION_NETWORK_PROFILE_SIGNATURE_DOMAIN,
        &payload_bytes,
    );
    let profile_bytes =
        serde_json::to_vec(&serde_json::to_value(SignedCollaborationNetworkProfile {
            payload: profile,
            signature,
            signer_did: envelope_signer,
        })?)?;
    let config = CollaborationStartupConfigFile {
        schema: COLLABORATION_STARTUP_CONFIG_SCHEMA.to_string(),
        expected_network_id: network_id.to_string(),
        trusted_profile_signer_dids: vec![signer_did],
        profile_chain_base64: vec![base64::engine::general_purpose::STANDARD.encode(profile_bytes)],
        default_conversation_grant_base64: Some(
            base64::engine::general_purpose::STANDARD.encode(grant_bytes),
        ),
    };
    let bytes = canonical_startup_config_bytes(&config)?;
    parse_and_validate_collaboration_startup_configuration(&bytes)?;
    Ok(bytes)
}

fn verified_receipt(config_bytes: &[u8]) -> anyhow::Result<OperatorReceipt> {
    let validated = parse_and_validate_collaboration_startup_configuration(config_bytes)?;
    let profile = validated
        .network()
        .head()
        .context("validated collaboration profile chain is empty")?;
    let grant = validated
        .network()
        .grant()
        .context("offline collaboration configuration requires a default-conversation grant")?;
    let peer = profile
        .profile()
        .bootstrap_peers
        .as_slice()
        .first()
        .filter(|_| profile.profile().bootstrap_peers.len() == 1)
        .context("offline collaboration configuration requires exactly one bootstrap peer")?;
    Ok(OperatorReceipt {
        schema: OPERATOR_RECEIPT_SCHEMA,
        network_id: profile.profile().network_id.clone(),
        conversation_id: grant.grant().conversation_id.clone(),
        signer_did: profile.profile().signer_did.clone(),
        profile_sha256: profile.profile_sha256().to_string(),
        grant_cid: grant.grant_cid().to_string(),
        bootstrap_node_id: peer.node_id.clone(),
        config_sha256: sha256_label(config_bytes),
    })
}

fn read_authority_key(path: &Path) -> anyhow::Result<SigningKey> {
    let bytes = read_owner_only_file(path, 32, "collaboration authority key")?;
    let key: [u8; 32] = bytes.try_into().map_err(|_| {
        anyhow::anyhow!("collaboration authority key must contain exactly 32 bytes")
    })?;
    Ok(SigningKey::from_bytes(&key))
}

fn read_owner_only_file(path: &Path, max_bytes: usize, label: &str) -> anyhow::Result<Vec<u8>> {
    validate_owner_only_parent(path)?;
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("failed to inspect {label}"))?;
    validate_owner_only_regular_file(path, &metadata, label)?;
    let metadata_len = usize::try_from(metadata.len())
        .with_context(|| format!("{label} length does not fit memory bounds"))?;
    if metadata_len > max_bytes {
        anyhow::bail!("{label} exceeds its byte limit");
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    validate_owner_only_regular_file(path, &file.metadata()?, label)?;
    let read_limit = u64::try_from(max_bytes)?
        .checked_add(1)
        .context("collaboration operator read bound overflow")?;
    let mut bytes = Vec::with_capacity(metadata_len);
    file.take(read_limit).read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        anyhow::bail!("{label} exceeds its byte limit");
    }
    Ok(bytes)
}

fn create_owner_only_file(path: &Path, bytes: &[u8], label: &str) -> anyhow::Result<()> {
    let parent = validate_owner_only_parent(path)?;
    if fs::symlink_metadata(path).is_ok() {
        anyhow::bail!("{label} output already exists");
    }
    let temp_path = parent.join(format!(".collaboration-config.{}.tmp", random_hex_128()?));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = options.open(&temp_path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        validate_owner_only_regular_file(&temp_path, &file.metadata()?, label)?;
        fs::hard_link(&temp_path, path).with_context(|| format!("failed to create {label}"))?;
        validate_owner_only_regular_file(path, &fs::symlink_metadata(path)?, label)?;
        fs::remove_file(&temp_path)?;
        File::open(&parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn validate_owner_only_parent(path: &Path) -> anyhow::Result<PathBuf> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let metadata = fs::symlink_metadata(parent)
        .with_context(|| "collaboration operator output parent does not exist")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("collaboration operator path parent must be a real directory");
    }
    Ok(parent.to_path_buf())
}

fn validate_private_output_target(path: &Path) -> anyhow::Result<()> {
    let parent = validate_owner_only_parent(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = fs::metadata(parent)?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            anyhow::bail!("collaboration bootstrap output parent must be owner-only");
        }
    }
    if fs::symlink_metadata(path).is_ok() {
        anyhow::bail!("collaboration bootstrap receipt output already exists");
    }
    Ok(())
}

fn validate_owner_only_regular_file(
    path: &Path,
    metadata: &fs::Metadata,
    label: &str,
) -> anyhow::Result<()> {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        anyhow::bail!("{label} must be a regular non-symlink file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            anyhow::bail!("{label} must be owner-only: {}", path.display());
        }
    }
    Ok(())
}

fn random_hex_128() -> anyhow::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .context("OS randomness unavailable for collaboration operator file")?;
    Ok(hex::encode(bytes))
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

fn write_receipt(output: &mut dyn Write, receipt: &impl Serialize) -> anyhow::Result<()> {
    serde_json::to_writer(&mut *output, receipt)?;
    output.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::provider::{
        Provider, ProviderError, ProviderRegistry, ResourceRequest, ResourceResponse,
    };
    use std::sync::Arc;

    struct BootstrapTicketProvider(CollaborationBootstrapPeer);

    #[async_trait::async_trait]
    impl Provider for BootstrapTicketProvider {
        async fn handle(
            &self,
            _request: ResourceRequest,
        ) -> Result<ResourceResponse, ProviderError> {
            Err(ProviderError::Provider("raw requests only".to_string()))
        }

        fn schemes(&self) -> Vec<&'static str> {
            vec!["elastos"]
        }

        fn name(&self) -> &'static str {
            "bootstrap-ticket-test"
        }

        async fn send_raw(
            &self,
            request: &serde_json::Value,
        ) -> Result<serde_json::Value, ProviderError> {
            if request != &serde_json::json!({"op": "get_ticket"}) {
                return Err(ProviderError::Provider("unexpected operation".to_string()));
            }
            Ok(serde_json::json!({
                "status": "ok",
                "data": {
                    "ticket": self.0.connect_ticket,
                    "node_id": self.0.node_id,
                }
            }))
        }
    }

    fn bootstrap_peer(secret_byte: u8) -> CollaborationBootstrapPeer {
        let secret = iroh::SecretKey::from_bytes(&[secret_byte; 32]);
        let endpoint = iroh::EndpointAddr::from(secret.public());
        let ticket_bytes = serde_json::to_vec(&serde_json::json!({
            "topic": null,
            "endpoints": [endpoint],
        }))
        .unwrap();
        let mut connect_ticket = data_encoding::BASE32_NOPAD.encode(&ticket_bytes);
        connect_ticket.make_ascii_lowercase();
        CollaborationBootstrapPeer {
            node_id: secret.public().to_string(),
            connect_ticket,
        }
    }

    fn write_owner_only(path: &Path, bytes: &[u8]) {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn canonical_peer_bytes(peer: &CollaborationBootstrapPeer) -> Vec<u8> {
        serde_json::to_vec(&serde_json::to_value(peer).unwrap()).unwrap()
    }

    fn assert_one_json_receipt(bytes: &[u8]) {
        let text = std::str::from_utf8(bytes).unwrap();
        assert!(text.ends_with('\n'));
        let mut lines = text.lines();
        serde_json::from_str::<serde_json::Value>(lines.next().unwrap()).unwrap();
        assert!(lines.next().is_none());
    }

    fn valid_config() -> (SigningKey, CollaborationBootstrapPeer, Vec<u8>) {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let peer = bootstrap_peer(9);
        let bytes = initial_config_bytes(
            &key,
            "operator-test-network",
            "operator-test-conversation",
            peer.clone(),
        )
        .unwrap();
        (key, peer, bytes)
    }

    #[tokio::test]
    async fn deterministic_golden_generation_and_pure_round_trip() {
        let (key, peer, expected) = valid_config();
        assert_eq!(
            sha256_label(&expected),
            "sha256:4f7ba738bb927b8f030696ce2b18d12ea78ffaa47f6d6a6524699e6632bc50f8"
        );
        assert_eq!(
            expected,
            initial_config_bytes(
                &key,
                "operator-test-network",
                "operator-test-conversation",
                peer.clone(),
            )
            .unwrap()
        );

        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("authority.key");
        let peer_path = temp.path().join("bootstrap.json");
        let config_path = temp.path().join("collaboration-network-v1.json");
        write_owner_only(&key_path, &key.to_bytes());
        write_owner_only(&peer_path, &canonical_peer_bytes(&peer));
        let mut generated_receipt = Vec::new();
        run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::GenerateInitial {
                authority_key: key_path,
                network_id: "operator-test-network".to_string(),
                conversation_id: "operator-test-conversation".to_string(),
                bootstrap_peer: peer_path,
                output: config_path.clone(),
            },
            &mut generated_receipt,
        )
        .await
        .unwrap();
        assert_one_json_receipt(&generated_receipt);
        assert_eq!(fs::read(&config_path).unwrap(), expected);

        let mut verified_receipt = Vec::new();
        run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::Verify { input: config_path },
            &mut verified_receipt,
        )
        .await
        .unwrap();
        assert_one_json_receipt(&verified_receipt);
        assert_eq!(generated_receipt, verified_receipt);
        let receipt = String::from_utf8(verified_receipt).unwrap();
        assert!(!receipt.contains(&peer.connect_ticket));
        assert!(!receipt.contains(&hex::encode(key.to_bytes())));
        assert!(
            !receipt.contains(&base64::engine::general_purpose::STANDARD.encode(key.to_bytes()))
        );
        assert!(!temp
            .path()
            .join("collaboration/config/accepted-profile-head-v1.json")
            .exists());
        assert!(!temp.path().join("device.key").exists());
        assert!(!temp.path().join("identity/device.key").exists());
        assert!(!temp.path().join("chat").exists());
        assert!(!temp.path().join("room").exists());
        let isolated = tempfile::tempdir().unwrap();
        crate::collaboration_startup::load_and_accept_collaboration_startup_configuration(
            isolated.path(),
        )
        .unwrap();
        assert_eq!(fs::read_dir(isolated.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn key_and_config_files_are_owner_only_no_follow_and_create_new() {
        let temp = tempfile::tempdir().unwrap();
        let key_path = temp.path().join("authority.key");
        let mut receipt = Vec::new();
        run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::CreateAuthorityKey {
                key: key_path.clone(),
            },
            &mut receipt,
        )
        .await
        .unwrap();
        assert_one_json_receipt(&receipt);
        let key_bytes = fs::read(&key_path).unwrap();
        assert_eq!(key_bytes.len(), 32);
        let key_receipt = String::from_utf8(receipt).unwrap();
        assert!(!key_receipt.contains(&hex::encode(&key_bytes)));
        assert!(
            !key_receipt.contains(&base64::engine::general_purpose::STANDARD.encode(&key_bytes))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
            assert_eq!(fs::metadata(&key_path).unwrap().mode() & 0o777, 0o600);
            assert!(run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::CreateAuthorityKey {
                    key: key_path.clone(),
                },
                &mut Vec::new(),
            )
            .await
            .is_err());

            let peer = bootstrap_peer(9);
            let peer_path = temp.path().join("bootstrap.json");
            write_owner_only(&peer_path, &canonical_peer_bytes(&peer));
            let config_path = temp.path().join("generated-config.json");
            run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::GenerateInitial {
                    authority_key: key_path.clone(),
                    network_id: "operator-test-network".to_string(),
                    conversation_id: "operator-test-conversation".to_string(),
                    bootstrap_peer: peer_path.clone(),
                    output: config_path.clone(),
                },
                &mut Vec::new(),
            )
            .await
            .unwrap();
            assert_eq!(fs::metadata(&config_path).unwrap().mode() & 0o777, 0o600);
            assert!(run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::GenerateInitial {
                    authority_key: key_path.clone(),
                    network_id: "operator-test-network".to_string(),
                    conversation_id: "operator-test-conversation".to_string(),
                    bootstrap_peer: peer_path.clone(),
                    output: config_path.clone(),
                },
                &mut Vec::new(),
            )
            .await
            .is_err());

            let output_link = temp.path().join("linked-output.json");
            symlink(&config_path, &output_link).unwrap();
            assert!(run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::GenerateInitial {
                    authority_key: key_path.clone(),
                    network_id: "operator-test-network".to_string(),
                    conversation_id: "operator-test-conversation".to_string(),
                    bootstrap_peer: peer_path.clone(),
                    output: output_link,
                },
                &mut Vec::new(),
            )
            .await
            .is_err());

            fs::set_permissions(&peer_path, fs::Permissions::from_mode(0o644)).unwrap();
            let error = run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::GenerateInitial {
                    authority_key: key_path.clone(),
                    network_id: "operator-test-network".to_string(),
                    conversation_id: "operator-test-conversation".to_string(),
                    bootstrap_peer: peer_path,
                    output: temp.path().join("config.json"),
                },
                &mut Vec::new(),
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("owner-only"));

            let (_, _, config) = valid_config();
            let real = temp.path().join("real-config.json");
            write_owner_only(&real, &config);
            let linked = temp.path().join("linked-config.json");
            symlink(&real, &linked).unwrap();
            assert!(run_collaboration_config_command_with_writer(
                CollaborationConfigCommand::Verify { input: linked },
                &mut Vec::new(),
            )
            .await
            .is_err());

            let oversized = temp.path().join("oversized-config.json");
            write_owner_only(
                &oversized,
                &vec![b'x'; crate::collaboration_startup::MAX_STARTUP_CONFIG_BYTES + 1],
            );
            assert!(read_collaboration_startup_config_candidate(&oversized).is_err());
        }
    }

    #[tokio::test]
    async fn local_bootstrap_export_is_canonical_private_redacted_and_pre_effect() {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let peer = bootstrap_peer(21);
        let output_path = temp.path().join("bootstrap.json");
        let mut receipt = Vec::new();
        write_local_bootstrap_receipt(
            peer.connect_ticket.clone(),
            peer.node_id.clone(),
            &output_path,
            &mut receipt,
        )
        .unwrap();

        assert_eq!(fs::read(&output_path).unwrap(), canonical_peer_bytes(&peer));
        assert_one_json_receipt(&receipt);
        let receipt_text = String::from_utf8(receipt).unwrap();
        let receipt_value: serde_json::Value = serde_json::from_str(receipt_text.trim()).unwrap();
        assert_eq!(receipt_value.as_object().unwrap().len(), 4);
        assert_eq!(receipt_value["schema"], LOCAL_BOOTSTRAP_RECEIPT_SCHEMA);
        assert_eq!(receipt_value["node_id"], peer.node_id);
        assert_eq!(receipt_value["created"], true);
        assert_eq!(
            receipt_value["output_sha256"],
            sha256_label(&canonical_peer_bytes(&peer))
        );
        assert!(!receipt_text.contains(&peer.connect_ticket));
        assert!(!receipt_text.contains(output_path.to_string_lossy().as_ref()));
        assert!(!temp.path().join("identity/device.key").exists());
        assert!(!temp.path().join("chat").exists());
        assert!(!temp.path().join("room").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
            assert_eq!(fs::metadata(&output_path).unwrap().mode() & 0o777, 0o600);

            let original = fs::read(&output_path).unwrap();
            let replay_error = write_local_bootstrap_receipt(
                peer.connect_ticket.clone(),
                peer.node_id.clone(),
                &output_path,
                &mut Vec::new(),
            )
            .unwrap_err();
            assert_eq!(fs::read(&output_path).unwrap(), original);
            assert!(!format!("{replay_error:#}").contains(&peer.connect_ticket));

            let link_path = temp.path().join("bootstrap-link.json");
            symlink(&output_path, &link_path).unwrap();
            assert!(write_local_bootstrap_receipt(
                peer.connect_ticket.clone(),
                peer.node_id.clone(),
                &link_path,
                &mut Vec::new(),
            )
            .is_err());

            let open_parent = temp.path().join("open-parent");
            fs::create_dir(&open_parent).unwrap();
            fs::set_permissions(&open_parent, fs::Permissions::from_mode(0o755)).unwrap();
            let open_output = open_parent.join("bootstrap.json");
            assert!(write_local_bootstrap_receipt(
                peer.connect_ticket.clone(),
                peer.node_id.clone(),
                &open_output,
                &mut Vec::new(),
            )
            .is_err());
            assert!(!open_output.exists());
        }

        let mismatch_path = temp.path().join("mismatch.json");
        let mismatch_error = write_local_bootstrap_receipt(
            peer.connect_ticket.clone(),
            bootstrap_peer(22).node_id,
            &mismatch_path,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(!mismatch_path.exists());
        assert!(!format!("{mismatch_error:#}").contains(&peer.connect_ticket));

        let malformed_path = temp.path().join("malformed.json");
        assert!(write_local_bootstrap_receipt(
            "not-a-ticket".to_string(),
            peer.node_id,
            &malformed_path,
            &mut Vec::new(),
        )
        .is_err());
        assert!(!malformed_path.exists());

        let missing_runtime_root = tempfile::tempdir().unwrap();
        let missing_output = temp.path().join("missing-runtime.json");
        let error = run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::ExportLocalBootstrapReceipt {
                data_root: missing_runtime_root.path().to_path_buf(),
                runtime_kind: crate::runtime_control::AttachableRuntimeKind::Operator,
                output: missing_output.clone(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(!missing_output.exists());
        assert!(!format!("{error:#}").contains(missing_output.to_string_lossy().as_ref()));
        assert!(
            !format!("{error:#}").contains(missing_runtime_root.path().to_string_lossy().as_ref())
        );

        let stopped_runtime_root = tempfile::tempdir().unwrap();
        crate::runtime_control::write_runtime_coords(
            &crate::runtime_control::runtime_coord_path(stopped_runtime_root.path()),
            &crate::runtime_control::RuntimeCoords {
                api_url: "http://127.0.0.1:9".to_string(),
                attach_secret: "attach-secret-must-not-render".to_string(),
                pid: u32::MAX,
                runtime_kind: crate::runtime_control::RUNTIME_KIND_OPERATOR.to_string(),
                binary_sha256: String::new(),
                policy_sha256: String::new(),
                dependency_sha256: String::new(),
            },
        )
        .unwrap();
        let stopped_output = temp.path().join("stopped-runtime.json");
        let error = run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::ExportLocalBootstrapReceipt {
                data_root: stopped_runtime_root.path().to_path_buf(),
                runtime_kind: crate::runtime_control::AttachableRuntimeKind::Operator,
                output: stopped_output.clone(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(!stopped_output.exists());
        let error = format!("{error:#}");
        assert!(!error.contains("attach-secret-must-not-render"));
        assert!(!error.contains(stopped_runtime_root.path().to_string_lossy().as_ref()));
    }

    #[tokio::test]
    async fn gateway_bootstrap_export_uses_only_the_explicit_gateway_coordinate() {
        let temp = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let peer = bootstrap_peer(27);
        let registry = Arc::new(ProviderRegistry::new());
        registry
            .register_sub_provider("peer", Arc::new(BootstrapTicketProvider(peer.clone())))
            .await
            .unwrap();
        let control =
            crate::api::gateway_local_control::start_gateway_local_control(temp.path(), registry)
                .await
                .unwrap();

        let output_path = temp.path().join("gateway-bootstrap.json");
        let mut receipt = Vec::new();
        run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::ExportLocalBootstrapReceipt {
                data_root: temp.path().to_path_buf(),
                runtime_kind: crate::runtime_control::AttachableRuntimeKind::Gateway,
                output: output_path.clone(),
            },
            &mut receipt,
        )
        .await
        .unwrap();
        assert_eq!(fs::read(&output_path).unwrap(), canonical_peer_bytes(&peer));
        assert_one_json_receipt(&receipt);
        let receipt = String::from_utf8(receipt).unwrap();
        assert!(!receipt.contains(&peer.connect_ticket));
        assert!(!receipt.contains(temp.path().to_string_lossy().as_ref()));

        let operator_output = temp.path().join("operator-bootstrap.json");
        assert!(run_collaboration_config_command_with_writer(
            CollaborationConfigCommand::ExportLocalBootstrapReceipt {
                data_root: temp.path().to_path_buf(),
                runtime_kind: crate::runtime_control::AttachableRuntimeKind::Operator,
                output: operator_output.clone(),
            },
            &mut Vec::new(),
        )
        .await
        .is_err());
        assert!(!operator_output.exists());
        control.shutdown().await.unwrap();
    }

    #[test]
    fn substituted_authority_ticket_profile_and_grant_fail_closed_without_secret_output() {
        let (key, peer, config) = valid_config();
        let mut candidates = Vec::new();

        let mut noncanonical = config.clone();
        noncanonical.push(b' ');
        candidates.push(noncanonical);

        let mut wrong_network: serde_json::Value = serde_json::from_slice(&config).unwrap();
        wrong_network["expected_network_id"] = serde_json::json!("another-network");
        candidates.push(serde_json::to_vec(&wrong_network).unwrap());

        let (other_key, _) = elastos_runtime::signature::generate_keypair();
        let other_did = crate::crypto::encode_signing_key_did(&other_key);
        let mut wrong_signer: serde_json::Value = serde_json::from_slice(&config).unwrap();
        wrong_signer["trusted_profile_signer_dids"] = serde_json::json!([other_did]);
        candidates.push(serde_json::to_vec(&wrong_signer).unwrap());

        let mut bad_signature: CollaborationStartupConfigFile =
            serde_json::from_slice(&config).unwrap();
        let profile_bytes = base64::engine::general_purpose::STANDARD
            .decode(&bad_signature.profile_chain_base64[0])
            .unwrap();
        let mut profile: SignedCollaborationNetworkProfile =
            serde_json::from_slice(&profile_bytes).unwrap();
        let replacement = if profile.signature.starts_with("00") {
            "01"
        } else {
            "00"
        };
        profile.signature.replace_range(0..2, replacement);
        bad_signature.profile_chain_base64[0] = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&serde_json::to_value(profile).unwrap()).unwrap());
        candidates.push(canonical_startup_config_bytes(&bad_signature).unwrap());

        let mut wrong_grant: CollaborationStartupConfigFile =
            serde_json::from_slice(&config).unwrap();
        let grant = canonical_default_conversation_grant_bytes(&DefaultConversationGrant {
            schema: DEFAULT_CONVERSATION_GRANT_SCHEMA_V1.to_string(),
            network_id: "operator-test-network".to_string(),
            conversation_id: "substituted-conversation".to_string(),
            sender_service: CHAT_SERVICE.to_string(),
            admission_policy: DefaultConversationAdmissionPolicy::ProfileScopedSigner,
        })
        .unwrap();
        wrong_grant.default_conversation_grant_base64 =
            Some(base64::engine::general_purpose::STANDARD.encode(grant));
        candidates.push(canonical_startup_config_bytes(&wrong_grant).unwrap());

        for candidate in candidates {
            let error = verified_receipt(&candidate).unwrap_err();
            let text = format!("{error:#}");
            assert!(!text.contains(&peer.connect_ticket));
            assert!(!text.contains(&hex::encode(key.to_bytes())));
        }

        let mismatched = CollaborationBootstrapPeer {
            node_id: bootstrap_peer(10).node_id,
            connect_ticket: peer.connect_ticket.clone(),
        };
        let error = initial_config_bytes(
            &key,
            "operator-test-network",
            "operator-test-conversation",
            mismatched,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("node identity mismatch"));
        assert!(!format!("{error:#}").contains(&peer.connect_ticket));

        let malformed = CollaborationBootstrapPeer {
            node_id: peer.node_id.clone(),
            connect_ticket: "not-a-ticket".to_string(),
        };
        let error = initial_config_bytes(
            &key,
            "operator-test-network",
            "operator-test-conversation",
            malformed,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("canonical base32"));
        assert!(!format!("{error:#}").contains(&peer.connect_ticket));

        assert!(
            initial_config_bytes(&key, "operator-test-network", "invalid conversation", peer,)
                .is_err()
        );
    }
}
