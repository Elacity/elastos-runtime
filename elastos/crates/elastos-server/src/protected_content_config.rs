//! Offline operator provisioning for the protected-content installed
//! prerequisites: the policy authority key, per-host custody node state,
//! the signed owner-only 2-of-3 custody composition, and the private
//! Chain provider configuration.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use base64::Engine as _;
use clap::Subcommand;
use ed25519_dalek::{Signer as _, SigningKey};
use elastos_protected_content_contracts::{
    CanonicalContract, CustodyApprovedSuitesV1, CustodyCommitteeAuthorizationStatementV1,
    CustodyEpochIssuerKeyV1, CustodyEpochStatementV1, CustodyNodeIdentityV1,
    CustodyPoolFailureDomainIdV1, CustodyPoolMemberStateV1, CustodyPoolMemberV1,
    CustodyPoolOperatorIdV1, CustodyPoolStatementV1, NodeCustodyPublicKeyV1, NodePublicKey,
    RuntimeOperationIssuerKeyV1, ShareCoordinateV1, SignedCustodyCommitteeAuthorizationV1,
    SignedCustodyEpochV1, SignedCustodyPoolV1, ThresholdV1, CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::collaboration_config::{create_owner_only_file, read_owner_only_file, write_receipt};
use crate::protected_content_runtime::{
    decode_canonical_peer_did, inactive_custody_state_root, protected_content_root,
    runtime_custody_composition_config_path, validate_owner_only_directory,
    RuntimeCustodyCompositionConfigFile, RuntimeCustodyRouteBindingConfig,
    RuntimeCustodyRouteTransportConfig, CUSTODY_COMPOSITION_SCHEMA_V1,
};

const POLICY_AUTHORITY_KEY_RECEIPT_SCHEMA: &str =
    "elastos.protected-content.policy-authority-key-created/v1";
const CUSTODY_NODE_PROVISION_RECEIPT_SCHEMA: &str =
    "elastos.protected-content.custody-node-provisioned/v1";
pub(crate) const CUSTODY_NODE_DESCRIPTOR_SCHEMA_V1: &str =
    "elastos.protected-content.custody-node-descriptor/v1";
const CUSTODY_COMPOSITION_RECEIPT_SCHEMA: &str =
    "elastos.protected-content.custody-composition-generated/v1";
const CUSTODY_COMPOSITION_VERIFIED_RECEIPT_SCHEMA: &str =
    "elastos.protected-content.custody-composition-verified/v1";
const RUNTIME_ISSUER_RECEIPT_SCHEMA: &str = "elastos.protected-content.runtime-issuer/v1";
/// Mirrors the Runtime loader's byte cap for the composition file.
const MAX_CUSTODY_COMPOSITION_FILE_BYTES: usize = 64 * 1024;
const CUSTODY_OPERATOR_ID_DOMAIN: &str = "elastos.protected-content.custody-operator-id/v1";
const CUSTODY_FAILURE_DOMAIN_ID_DOMAIN: &str =
    "elastos.protected-content.custody-failure-domain-id/v1";
const MAX_CUSTODY_NODE_LABEL_BYTES: usize = 64;
const MAX_CUSTODY_NODE_DESCRIPTOR_BYTES: usize = 8 * 1024;
/// The composition validators hard-code a 2-of-3 committee; the ceremony keeps
/// the threshold in one place so a future k-of-n only changes this pair.
const CUSTODY_THRESHOLD_REQUIRED: u8 = 2;
const CUSTODY_THRESHOLD_TOTAL: u8 = 3;
const CHAIN_CONFIG_RECEIPT_SCHEMA: &str =
    "elastos.protected-content.chain-provider-config-generated/v1";
/// Facts proven in the deployment review for Base (chain id 8453); see
/// docs/CHAIN_PROVIDER.md ("has_access_by_content_id") and docs/PROTECTED_CONTENT.md.
const PROVEN_BASE_AUTHORITY_GATEWAY: &str = "0x09dbe796f40eceffeaccf243c3d758c4c1d8d87d";
const PROVEN_HAS_ACCESS_SELECTOR: &str = "0x54d42821";
/// Evidence corroboration needs 2..=5 independent sources; documented as the
/// product contract in docs/PROTECTED_CONTENT.md and enforced again by the
/// chain-provider capsule at Init.
const MIN_EVIDENCE_RPC_SOURCES: usize = 2;
const MAX_EVIDENCE_RPC_SOURCES: usize = 5;

#[derive(Debug, Subcommand)]
pub enum ProtectedContentConfigCommand {
    /// Create a new dedicated 32-byte protected-content policy authority key.
    CreatePolicyAuthorityKey {
        #[arg(long)]
        key: PathBuf,
    },
    /// Provision this host's inactive custody provider state root and export
    /// its custody node descriptor for the composition ceremony.
    ProvisionCustodyNode {
        #[arg(long)]
        data_dir: PathBuf,
        /// Runtime operation issuer this custody node will trust (0x + 64 lowercase hex).
        #[arg(long)]
        trusted_runtime_issuer: String,
        /// Operator label for this node (distinct across the three nodes).
        #[arg(long)]
        operator: String,
        /// Failure-domain label for this node (distinct across the three nodes).
        #[arg(long)]
        failure_domain: String,
        /// Carrier peer DID route to this node; omit for the single local route.
        #[arg(long)]
        transport_peer_did: Option<String>,
        #[arg(long)]
        output: PathBuf,
    },
    /// Assemble and sign the owner-only 2-of-3 custody composition from three
    /// custody node descriptors, installing it into the Runtime data dir.
    GenerateCustodyComposition {
        #[arg(long)]
        authority_key: PathBuf,
        /// Custody node descriptor path; pass exactly three.
        #[arg(long = "node")]
        nodes: Vec<PathBuf>,
        #[arg(long)]
        data_dir: PathBuf,
        /// Days the pool members stay active from now.
        #[arg(long, default_value_t = 365)]
        valid_days: u16,
    },
    /// Purely verify the installed custody composition through the Runtime loader.
    VerifyCustodyComposition {
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Print the Runtime operation issuer custody nodes must trust for this data dir.
    ShowRuntimeIssuer {
        #[arg(long)]
        data_dir: PathBuf,
    },
    /// Install the private multi-source Chain provider configuration for the
    /// protected-content rights, mint, and market methods.
    GenerateChainConfig {
        #[arg(long)]
        data_dir: PathBuf,
        #[arg(long, default_value = "base-mainnet")]
        id: String,
        #[arg(long, default_value = "Base")]
        display_name: String,
        #[arg(long, default_value_t = 8453)]
        chain_id: u64,
        #[arg(long, default_value = "ETH")]
        native_symbol: String,
        #[arg(long, default_value_t = true)]
        mainnet: bool,
        /// Primary JSON-RPC endpoint for transactions and reads.
        #[arg(long)]
        rpc_url: String,
        /// Independent evidence JSON-RPC endpoint (repeat 2..=5 times, distinct origins).
        #[arg(long = "evidence-rpc-url")]
        evidence_rpc_urls: Vec<String>,
        /// Rights gateway contract proven for this deployment.
        #[arg(long, default_value = PROVEN_BASE_AUTHORITY_GATEWAY)]
        rights_contract: String,
        /// has_access_by_content_id selector proven for this deployment.
        #[arg(long, default_value = PROVEN_HAS_ACCESS_SELECTOR)]
        rights_selector: String,
        #[arg(long, default_value = PROVEN_BASE_AUTHORITY_GATEWAY)]
        authority_gateway_contract: String,
        #[arg(long)]
        mint_ledger: String,
        #[arg(long)]
        mint_pay_token: String,
        #[arg(long)]
        mint_asset_created_emitter: String,
    },
}

#[derive(Serialize)]
struct PolicyAuthorityKeyReceipt {
    schema: &'static str,
    created: bool,
}

#[derive(Serialize)]
struct CustodyNodeProvisionReceipt {
    schema: &'static str,
    node_public_key_hex: String,
    descriptor_sha256: String,
    created: bool,
}

#[derive(Serialize)]
struct CustodyCompositionReceipt {
    schema: &'static str,
    node_public_key_hexes: Vec<String>,
    config_sha256: String,
    created: bool,
}

#[derive(Serialize)]
struct CustodyCompositionVerifiedReceipt {
    schema: &'static str,
    node_public_key_hexes: Vec<String>,
    config_sha256: String,
    verified: bool,
}

#[derive(Serialize)]
struct RuntimeIssuerReceipt {
    schema: &'static str,
    trusted_runtime_issuer: String,
}

#[derive(Serialize)]
struct ChainConfigReceipt {
    schema: &'static str,
    network_id: String,
    chain_id: u64,
    evidence_rpc_sources: usize,
    config_sha256: String,
    created: bool,
}

/// One custody node's public identity plus its operator-declared placement,
/// produced on the custody host and consumed by the composition ceremony.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CustodyNodeDescriptorV1 {
    pub(crate) schema: String,
    pub(crate) node_public_key_hex: String,
    pub(crate) node_custody_public_key_hex: String,
    pub(crate) operator: String,
    pub(crate) failure_domain: String,
    pub(crate) transport: CustodyNodeDescriptorTransportV1,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CustodyNodeDescriptorTransportV1 {
    Local,
    CarrierPeerDid { peer_did: String },
}

pub async fn run_protected_content_config_command(
    command: ProtectedContentConfigCommand,
) -> anyhow::Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    run_protected_content_config_command_with_writer(command, &mut output).await
}

async fn run_protected_content_config_command_with_writer(
    command: ProtectedContentConfigCommand,
    output: &mut dyn Write,
) -> anyhow::Result<()> {
    match command {
        ProtectedContentConfigCommand::CreatePolicyAuthorityKey { key } => {
            let mut bytes = [0u8; 32];
            getrandom::getrandom(&mut bytes)
                .context("OS randomness unavailable for protected-content policy authority key")?;
            create_owner_only_file(&key, &bytes, "protected-content policy authority key")?;
            write_receipt(
                output,
                &PolicyAuthorityKeyReceipt {
                    schema: POLICY_AUTHORITY_KEY_RECEIPT_SCHEMA,
                    created: true,
                },
            )
        }
        ProtectedContentConfigCommand::ProvisionCustodyNode {
            data_dir,
            trusted_runtime_issuer,
            operator,
            failure_domain,
            transport_peer_did,
            output: output_path,
        } => {
            let issuer = parse_runtime_issuer_hex(&trusted_runtime_issuer)?;
            let operator = validated_custody_node_label(&operator, "operator")?;
            let failure_domain = validated_custody_node_label(&failure_domain, "failure domain")?;
            let transport = match transport_peer_did {
                None => CustodyNodeDescriptorTransportV1::Local,
                Some(peer_did) => CustodyNodeDescriptorTransportV1::CarrierPeerDid {
                    peer_did: decode_canonical_peer_did(&peer_did)?,
                },
            };
            let inactive_root = provisioned_inactive_custody_root(&data_dir)?;
            let provisioned = custody_provider::provision_state_root(&inactive_root, issuer)
                .map_err(|error| {
                    anyhow::anyhow!("custody provider state provisioning failed: {error:?}")
                })?;
            let descriptor = CustodyNodeDescriptorV1 {
                schema: CUSTODY_NODE_DESCRIPTOR_SCHEMA_V1.to_string(),
                node_public_key_hex: format!(
                    "0x{}",
                    hex::encode(provisioned.node_public_key.as_bytes())
                ),
                node_custody_public_key_hex: format!(
                    "0x{}",
                    hex::encode(provisioned.node_custody_public_key.as_bytes())
                ),
                operator,
                failure_domain,
                transport,
            };
            let descriptor_bytes = serde_json::to_vec(&serde_json::to_value(&descriptor)?)?;
            create_owner_only_file(&output_path, &descriptor_bytes, "custody node descriptor")?;
            write_receipt(
                output,
                &CustodyNodeProvisionReceipt {
                    schema: CUSTODY_NODE_PROVISION_RECEIPT_SCHEMA,
                    node_public_key_hex: descriptor.node_public_key_hex,
                    descriptor_sha256: format!(
                        "sha256:{}",
                        hex::encode(Sha256::digest(&descriptor_bytes))
                    ),
                    created: true,
                },
            )
        }
        ProtectedContentConfigCommand::GenerateCustodyComposition {
            authority_key,
            nodes,
            data_dir,
            valid_days,
        } => {
            let authority = read_policy_authority_key(&authority_key)?;
            let descriptors = read_distinct_custody_node_descriptors(&nodes)?;
            let config = signed_custody_composition_config(&authority, descriptors, valid_days)?;
            let config_bytes = serde_json::to_vec(&serde_json::to_value(&config)?)?;

            validate_existing_data_dir(&data_dir)?;
            ensure_owner_only_dir(&protected_content_root(&data_dir), "protected-content root")?;
            let config_path = runtime_custody_composition_config_path(&data_dir);
            create_owner_only_file(&config_path, &config_bytes, "custody composition")?;

            // The Runtime loader is the single validation authority; prove the
            // installed file end to end before reporting success.
            if let Err(error) = crate::protected_content_runtime::load_runtime_custody_composition(
                &data_dir,
                std::sync::Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
            ) {
                let _ = std::fs::remove_file(&config_path);
                return Err(error.context("generated custody composition failed loader validation"));
            }

            write_receipt(
                output,
                &CustodyCompositionReceipt {
                    schema: CUSTODY_COMPOSITION_RECEIPT_SCHEMA,
                    node_public_key_hexes: config
                        .routes
                        .iter()
                        .map(|route| {
                            Ok(format!(
                                "0x{}",
                                hex::encode(decode_standard_base64(&route.node_public_key_base64)?)
                            ))
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?,
                    config_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&config_bytes))),
                    created: true,
                },
            )
        }
        ProtectedContentConfigCommand::VerifyCustodyComposition { data_dir } => {
            crate::protected_content_runtime::load_runtime_custody_composition(
                &data_dir,
                std::sync::Arc::new(elastos_runtime::provider::ProviderRegistry::new()),
            )?
            .context("custody composition is missing; generate and install it first")?;
            let config_bytes = read_owner_only_file(
                &runtime_custody_composition_config_path(&data_dir),
                MAX_CUSTODY_COMPOSITION_FILE_BYTES,
                "custody composition",
            )?;
            let config: RuntimeCustodyCompositionConfigFile =
                serde_json::from_slice(&config_bytes)?;
            write_receipt(
                output,
                &CustodyCompositionVerifiedReceipt {
                    schema: CUSTODY_COMPOSITION_VERIFIED_RECEIPT_SCHEMA,
                    node_public_key_hexes: config
                        .routes
                        .iter()
                        .map(|route| {
                            Ok(format!(
                                "0x{}",
                                hex::encode(decode_standard_base64(&route.node_public_key_base64)?)
                            ))
                        })
                        .collect::<anyhow::Result<Vec<_>>>()?,
                    config_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&config_bytes))),
                    verified: true,
                },
            )
        }
        ProtectedContentConfigCommand::ShowRuntimeIssuer { data_dir } => {
            let device_key_path = elastos_identity::device_key_path(&data_dir);
            if std::fs::symlink_metadata(&device_key_path).is_err() {
                anyhow::bail!(
                    "Runtime device key is missing at {}; start the Runtime on this data dir once, then retry",
                    device_key_path.display()
                );
            }
            let device_key = elastos_identity::load_or_create_device_key(&data_dir)?;
            let issuer = crate::protected_content_runtime::derive_protected_content_runtime_issuer(
                &device_key,
            )?;
            write_receipt(
                output,
                &RuntimeIssuerReceipt {
                    schema: RUNTIME_ISSUER_RECEIPT_SCHEMA,
                    trusted_runtime_issuer: format!("0x{}", hex::encode(issuer.as_bytes())),
                },
            )
        }
        ProtectedContentConfigCommand::GenerateChainConfig {
            data_dir,
            id,
            display_name,
            chain_id,
            native_symbol,
            mainnet,
            rpc_url,
            evidence_rpc_urls,
            rights_contract,
            rights_selector,
            authority_gateway_contract,
            mint_ledger,
            mint_pay_token,
            mint_asset_created_emitter,
        } => {
            validate_private_rpc_url(&rpc_url, "rpc url")?;
            validate_evidence_rpc_urls(&evidence_rpc_urls)?;
            let rights_contract = validated_contract_address(&rights_contract, "rights contract")?;
            let authority_gateway_contract = validated_contract_address(
                &authority_gateway_contract,
                "authority gateway contract",
            )?;
            let mint_ledger = validated_contract_address(&mint_ledger, "mint ledger")?;
            let mint_pay_token = validated_contract_address(&mint_pay_token, "mint pay token")?;
            let mint_asset_created_emitter = validated_contract_address(
                &mint_asset_created_emitter,
                "mint asset-created emitter",
            )?;
            validated_selector(&rights_selector)?;

            let network = serde_json::json!({
                "id": id,
                "display_name": display_name,
                "kind": "evm_json_rpc",
                "chain_id": chain_id,
                "native_symbol": native_symbol,
                "provider": "operator",
                "mainnet": mainnet,
                "explorer_url": null,
                "rpc_url": rpc_url,
                "rights_methods": [{
                    "id": "has_access_by_content_id",
                    "contract": rights_contract,
                    "abi": "has_access_by_content_id_address_bytes16",
                    "selector": rights_selector,
                    "protected_content_policies": [{
                        "action": "view",
                        "evidence_rpc_urls": evidence_rpc_urls,
                    }],
                }],
                "protected_content_creator_mint": {
                    "ledger": mint_ledger,
                    "pay_token": mint_pay_token,
                    "asset_created_emitter": mint_asset_created_emitter,
                    "abi": "elacity_mint_v1",
                },
                "protected_content_market": {
                    "authority_gateway_contract": authority_gateway_contract,
                    "evidence_rpc_urls": evidence_rpc_urls,
                },
            });
            let config_bytes = serde_json::to_vec(&serde_json::json!({
                "schema": crate::protected_content_runtime::PROTECTED_CONTENT_CHAIN_PROVIDER_CONFIG_SCHEMA_V1,
                "protected_content_network": network,
            }))?;

            validate_existing_data_dir(&data_dir)?;
            ensure_owner_only_dir(&protected_content_root(&data_dir), "protected-content root")?;
            let config_path = protected_content_root(&data_dir).join("chain-provider.json");
            create_owner_only_file(
                &config_path,
                &config_bytes,
                "protected-content chain config",
            )?;

            // Prove the installed file passes the Runtime loader before
            // reporting success; the chain-provider capsule re-validates the
            // network body at Init and stays the live admission authority.
            if let Err(error) =
                crate::protected_content_runtime::load_runtime_protected_content_chain_provider_config(
                    &data_dir,
                )
            {
                let _ = std::fs::remove_file(&config_path);
                return Err(error.context("generated chain config failed loader validation"));
            }

            write_receipt(
                output,
                &ChainConfigReceipt {
                    schema: CHAIN_CONFIG_RECEIPT_SCHEMA,
                    network_id: id,
                    chain_id,
                    evidence_rpc_sources: evidence_rpc_urls.len(),
                    config_sha256: format!("sha256:{}", hex::encode(Sha256::digest(&config_bytes))),
                    created: true,
                },
            )
        }
    }
}

/// Doc-backed shape rule for private chain endpoints (see the chain-provider
/// capsule's admission checks): https, or http against loopback only, and no
/// credentials in the URL.
fn validate_private_rpc_url(value: &str, label: &str) -> anyhow::Result<url::Url> {
    let parsed = url::Url::parse(value).with_context(|| format!("{label} is not a valid URL"))?;
    let loopback_http = parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1") | Some("localhost"));
    if parsed.scheme() != "https" && !loopback_http {
        anyhow::bail!("{label} must use https (or http against 127.0.0.1/localhost)");
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("{label} must not embed credentials");
    }
    Ok(parsed)
}

fn validate_evidence_rpc_urls(urls: &[String]) -> anyhow::Result<()> {
    if !(MIN_EVIDENCE_RPC_SOURCES..=MAX_EVIDENCE_RPC_SOURCES).contains(&urls.len()) {
        anyhow::bail!(
            "evidence corroboration requires {MIN_EVIDENCE_RPC_SOURCES}..={MAX_EVIDENCE_RPC_SOURCES} independent evidence RPC sources"
        );
    }
    let mut seen_urls = std::collections::BTreeSet::new();
    let mut seen_origins = std::collections::BTreeSet::new();
    for value in urls {
        let parsed = validate_private_rpc_url(value, "evidence rpc url")?;
        if !seen_urls.insert(parsed.to_string()) {
            anyhow::bail!("evidence RPC sources must be distinct URLs");
        }
        if !seen_origins.insert(parsed.origin().ascii_serialization()) {
            anyhow::bail!("evidence RPC sources must come from distinct origins");
        }
    }
    Ok(())
}

fn validated_contract_address(value: &str, label: &str) -> anyhow::Result<String> {
    let hex_part = value
        .strip_prefix("0x")
        .with_context(|| format!("{label} must start with 0x"))?;
    if hex_part.len() != 40
        || !hex_part
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        anyhow::bail!("{label} must be 0x plus 40 lowercase hex characters");
    }
    Ok(value.to_string())
}

fn validated_selector(value: &str) -> anyhow::Result<()> {
    let hex_part = value
        .strip_prefix("0x")
        .context("selector must start with 0x")?;
    if hex_part.len() != 8
        || !hex_part
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        anyhow::bail!("selector must be 0x plus 8 lowercase hex characters");
    }
    Ok(())
}

struct ValidatedCustodyNodeDescriptor {
    node_public_key: NodePublicKey,
    custody_public_key: NodeCustodyPublicKeyV1,
    operator: String,
    failure_domain: String,
    transport: RuntimeCustodyRouteTransportConfig,
}

fn read_policy_authority_key(path: &Path) -> anyhow::Result<SigningKey> {
    let bytes = read_owner_only_file(path, 32, "protected-content policy authority key")?;
    let key: [u8; 32] = bytes.try_into().map_err(|_| {
        anyhow::anyhow!("protected-content policy authority key must contain exactly 32 bytes")
    })?;
    Ok(SigningKey::from_bytes(&key))
}

fn read_distinct_custody_node_descriptors(
    paths: &[PathBuf],
) -> anyhow::Result<Vec<ValidatedCustodyNodeDescriptor>> {
    if paths.len() != usize::from(CUSTODY_THRESHOLD_TOTAL) {
        anyhow::bail!(
            "custody composition requires exactly {CUSTODY_THRESHOLD_TOTAL} --node descriptors"
        );
    }
    let mut descriptors = paths
        .iter()
        .map(|path| read_custody_node_descriptor(path))
        .collect::<anyhow::Result<Vec<_>>>()?;
    require_distinct(
        descriptors
            .iter()
            .map(|d| d.node_public_key.as_bytes().to_vec()),
        "node public keys",
    )?;
    require_distinct(
        descriptors
            .iter()
            .map(|d| d.custody_public_key.as_bytes().to_vec()),
        "node custody public keys",
    )?;
    require_distinct(
        descriptors.iter().map(|d| d.operator.clone().into_bytes()),
        "operator labels",
    )?;
    require_distinct(
        descriptors
            .iter()
            .map(|d| d.failure_domain.clone().into_bytes()),
        "failure-domain labels",
    )?;
    let local_routes = descriptors
        .iter()
        .filter(|d| matches!(d.transport, RuntimeCustodyRouteTransportConfig::Local))
        .count();
    if local_routes > 1 {
        anyhow::bail!("custody composition allows at most one local transport route");
    }
    require_distinct(
        descriptors.iter().filter_map(|d| match &d.transport {
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid { peer_did } => {
                Some(peer_did.clone().into_bytes())
            }
            RuntimeCustodyRouteTransportConfig::Local => None,
        }),
        "carrier peer DIDs",
    )?;
    // The epoch statement orders nodes by node public key; keep every derived
    // artifact (members, routes) in that one canonical order.
    descriptors.sort_by(|a, b| {
        a.node_public_key
            .as_bytes()
            .cmp(b.node_public_key.as_bytes())
    });
    Ok(descriptors)
}

fn read_custody_node_descriptor(path: &Path) -> anyhow::Result<ValidatedCustodyNodeDescriptor> {
    let bytes = read_owner_only_file(
        path,
        MAX_CUSTODY_NODE_DESCRIPTOR_BYTES,
        "custody node descriptor",
    )?;
    let descriptor: CustodyNodeDescriptorV1 = serde_json::from_slice(&bytes)
        .with_context(|| format!("invalid custody node descriptor at {}", path.display()))?;
    if serde_json::to_vec(&serde_json::to_value(&descriptor)?)? != bytes {
        anyhow::bail!(
            "custody node descriptor at {} is not canonical JSON",
            path.display()
        );
    }
    if descriptor.schema != CUSTODY_NODE_DESCRIPTOR_SCHEMA_V1 {
        anyhow::bail!(
            "custody node descriptor at {} carries an unsupported schema",
            path.display()
        );
    }
    let node_public_key = NodePublicKey::new(parse_prefixed_hex::<32>(
        &descriptor.node_public_key_hex,
        "node public key",
    )?)
    .map_err(|error| anyhow::anyhow!("custody node public key is invalid: {error:?}"))?;
    let custody_public_key = NodeCustodyPublicKeyV1::new(parse_prefixed_hex::<
        PQ_HYBRID_WRAP_PUBLIC_KEY_BYTES,
    >(
        &descriptor.node_custody_public_key_hex,
        "node custody public key",
    )?)
    .map_err(|error| anyhow::anyhow!("custody node custody public key is invalid: {error:?}"))?;
    let operator = validated_custody_node_label(&descriptor.operator, "operator")?;
    let failure_domain =
        validated_custody_node_label(&descriptor.failure_domain, "failure domain")?;
    let transport = match descriptor.transport {
        CustodyNodeDescriptorTransportV1::Local => RuntimeCustodyRouteTransportConfig::Local,
        CustodyNodeDescriptorTransportV1::CarrierPeerDid { peer_did } => {
            RuntimeCustodyRouteTransportConfig::CarrierPeerDid {
                peer_did: decode_canonical_peer_did(&peer_did)?,
            }
        }
    };
    Ok(ValidatedCustodyNodeDescriptor {
        node_public_key,
        custody_public_key,
        operator,
        failure_domain,
        transport,
    })
}

fn signed_custody_composition_config(
    authority: &SigningKey,
    descriptors: Vec<ValidatedCustodyNodeDescriptor>,
    valid_days: u16,
) -> anyhow::Result<RuntimeCustodyCompositionConfigFile> {
    let issuer = CustodyEpochIssuerKeyV1::new(authority.verifying_key().to_bytes())
        .map_err(|error| anyhow::anyhow!("policy authority key is invalid: {error:?}"))?;
    let suites = CustodyApprovedSuitesV1::new(
        CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
        CUSTODY_X_WING_AES256GCM_SUITE_ID_V1,
    )
    .map_err(|error| anyhow::anyhow!("approved custody suites are invalid: {error:?}"))?;

    let node_identities = descriptors
        .iter()
        .enumerate()
        .map(|(index, descriptor)| {
            let coordinate = ShareCoordinateV1::new(u8::try_from(index + 1)?)
                .map_err(|error| anyhow::anyhow!("share coordinate is invalid: {error:?}"))?;
            CustodyNodeIdentityV1::new(
                descriptor.node_public_key,
                descriptor.custody_public_key,
                coordinate,
            )
            .map_err(|error| anyhow::anyhow!("custody node identity is invalid: {error:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let epoch_statement = CustodyEpochStatementV1::new(
        issuer,
        suites.clone(),
        ThresholdV1::new(CUSTODY_THRESHOLD_REQUIRED, CUSTODY_THRESHOLD_TOTAL)
            .map_err(|error| anyhow::anyhow!("custody threshold is invalid: {error:?}"))?,
        node_identities,
    )
    .map_err(|error| anyhow::anyhow!("custody epoch statement is invalid: {error:?}"))?;
    let signed_epoch = SignedCustodyEpochV1::new(
        epoch_statement.clone(),
        authority
            .sign(&epoch_statement.canonical_bytes().map_err(|error| {
                anyhow::anyhow!("custody epoch statement cannot be encoded: {error:?}")
            })?)
            .to_bytes()
            .to_vec(),
    )
    .map_err(|error| anyhow::anyhow!("signed custody epoch is invalid: {error:?}"))?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let active_window = (
        now.saturating_sub(60),
        now.checked_add(u64::from(valid_days) * 86_400)
            .context("custody validity window overflows")?,
    );
    let members = descriptors
        .iter()
        .map(|descriptor| {
            CustodyPoolMemberV1::new(
                descriptor.node_public_key,
                descriptor.custody_public_key,
                CustodyPoolOperatorIdV1::new(custody_label_id(
                    CUSTODY_OPERATOR_ID_DOMAIN,
                    &descriptor.operator,
                )),
                CustodyPoolFailureDomainIdV1::new(custody_label_id(
                    CUSTODY_FAILURE_DOMAIN_ID_DOMAIN,
                    &descriptor.failure_domain,
                )),
                suites.clone(),
                active_window,
                CustodyPoolMemberStateV1::Active,
            )
            .map_err(|error| anyhow::anyhow!("custody pool member is invalid: {error:?}"))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let pool_statement = CustodyPoolStatementV1::new(issuer, members)
        .map_err(|error| anyhow::anyhow!("custody pool statement is invalid: {error:?}"))?;
    let signed_pool = SignedCustodyPoolV1::new(
        pool_statement.clone(),
        authority
            .sign(&pool_statement.canonical_bytes().map_err(|error| {
                anyhow::anyhow!("custody pool statement cannot be encoded: {error:?}")
            })?)
            .to_bytes()
            .to_vec(),
    )
    .map_err(|error| anyhow::anyhow!("signed custody pool is invalid: {error:?}"))?;

    let authorization_statement = CustodyCommitteeAuthorizationStatementV1::new(
        issuer,
        signed_pool
            .pool_identity()
            .map_err(|error| anyhow::anyhow!("custody pool identity is invalid: {error:?}"))?,
        signed_epoch
            .epoch_identity()
            .map_err(|error| anyhow::anyhow!("custody epoch identity is invalid: {error:?}"))?,
    )
    .map_err(|error| {
        anyhow::anyhow!("custody committee authorization statement is invalid: {error:?}")
    })?;
    let signed_authorization = SignedCustodyCommitteeAuthorizationV1::new(
        authorization_statement.clone(),
        authority
            .sign(&authorization_statement.canonical_bytes().map_err(|error| {
                anyhow::anyhow!("committee authorization cannot be encoded: {error:?}")
            })?)
            .to_bytes()
            .to_vec(),
    )
    .map_err(|error| anyhow::anyhow!("signed committee authorization is invalid: {error:?}"))?;

    let routes = descriptors
        .iter()
        .map(|descriptor| {
            Ok(RuntimeCustodyRouteBindingConfig {
                node_public_key_base64: encode_standard_base64(
                    descriptor.node_public_key.as_bytes(),
                ),
                owner_state_root_base64: encode_standard_base64(&random_owner_state_root()?),
                transport: descriptor.transport.clone(),
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(RuntimeCustodyCompositionConfigFile {
        schema: CUSTODY_COMPOSITION_SCHEMA_V1.to_string(),
        expected_policy_authority_base64: encode_standard_base64(
            &authority.verifying_key().to_bytes(),
        ),
        expected_committee_authorization_identity_base64: canonical_contract_base64(
            &signed_authorization
                .authorization_identity()
                .map_err(|error| {
                    anyhow::anyhow!("committee authorization identity is invalid: {error:?}")
                })?,
        )?,
        signed_pool_base64: canonical_contract_base64(&signed_pool)?,
        signed_epoch_base64: canonical_contract_base64(&signed_epoch)?,
        signed_committee_authorization_base64: canonical_contract_base64(&signed_authorization)?,
        routes,
    })
}

fn require_distinct(values: impl Iterator<Item = Vec<u8>>, label: &str) -> anyhow::Result<()> {
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            anyhow::bail!("custody composition requires distinct {label} across nodes");
        }
    }
    Ok(())
}

fn custody_label_id(domain: &str, label: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(domain.as_bytes());
    hasher.update([0u8]);
    hasher.update(label.as_bytes());
    hasher.finalize().into()
}

fn random_owner_state_root() -> anyhow::Result<[u8; 32]> {
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).context("OS randomness unavailable for owner state root")?;
    if bytes == [0u8; 32] {
        anyhow::bail!("owner state root randomness returned all zeroes");
    }
    Ok(bytes)
}

fn parse_prefixed_hex<const LEN: usize>(value: &str, label: &str) -> anyhow::Result<[u8; LEN]> {
    let hex_part = value
        .strip_prefix("0x")
        .with_context(|| format!("{label} must start with 0x"))?;
    if hex_part.len() != LEN * 2
        || !hex_part
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        anyhow::bail!(
            "{label} must be 0x plus {} lowercase hex characters",
            LEN * 2
        );
    }
    hex::decode(hex_part)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("{label} must encode {LEN} bytes"))
}

fn encode_standard_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn decode_standard_base64(value: &str) -> anyhow::Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .context("invalid base64 value")
}

fn canonical_contract_base64<T: CanonicalContract>(value: &T) -> anyhow::Result<String> {
    Ok(encode_standard_base64(&value.canonical_bytes().map_err(
        |error| anyhow::anyhow!("canonical contract encoding failed: {error:?}"),
    )?))
}

/// Create (if missing) and validate the owner-only directory chain that must
/// exist before the inactive custody provider state root can be provisioned
/// or registered at boot.
fn provisioned_inactive_custody_root(data_dir: &Path) -> anyhow::Result<PathBuf> {
    validate_existing_data_dir(data_dir)?;
    let protected_root = protected_content_root(data_dir);
    ensure_owner_only_dir(&protected_root, "protected-content root")?;
    let inactive_root = inactive_custody_state_root(data_dir);
    let custody_dir = inactive_root
        .parent()
        .context("inactive custody provider root has no parent directory")?;
    ensure_owner_only_dir(custody_dir, "custody provider root")?;
    Ok(inactive_root)
}

/// The Runtime enforces owner-only modes from `protected-content` down; the
/// data root itself only needs to be a real directory.
fn validate_existing_data_dir(data_dir: &Path) -> anyhow::Result<()> {
    let metadata = std::fs::symlink_metadata(data_dir)
        .with_context(|| format!("data dir {} is unavailable", data_dir.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        anyhow::bail!("data dir {} must be a real directory", data_dir.display());
    }
    Ok(())
}

fn ensure_owner_only_dir(path: &Path, label: &str) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .mode(0o700)
                    .create(path)
                    .with_context(|| format!("failed to create {label}"))?;
            }
            #[cfg(not(unix))]
            anyhow::bail!("{label} provisioning requires a Unix host");
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {label}"));
        }
        Ok(_) => {}
    }
    validate_owner_only_directory(path, label)
}

fn parse_runtime_issuer_hex(value: &str) -> anyhow::Result<RuntimeOperationIssuerKeyV1> {
    let hex_part = value
        .strip_prefix("0x")
        .context("trusted runtime issuer must start with 0x")?;
    if hex_part.len() != 64
        || !hex_part
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        anyhow::bail!("trusted runtime issuer must be 0x plus 64 lowercase hex characters");
    }
    let bytes: [u8; 32] = hex::decode(hex_part)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("trusted runtime issuer must encode 32 bytes"))?;
    RuntimeOperationIssuerKeyV1::new(bytes)
        .map_err(|_| anyhow::anyhow!("trusted runtime issuer is not a valid operation issuer key"))
}

fn validated_custody_node_label(value: &str, label: &str) -> anyhow::Result<String> {
    if value.is_empty() || value.trim() != value || value.len() > MAX_CUSTODY_NODE_LABEL_BYTES {
        anyhow::bail!(
            "custody node {label} label must be 1..={MAX_CUSTODY_NODE_LABEL_BYTES} bytes without surrounding whitespace"
        );
    }
    Ok(value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn create_policy_authority_key_creates_owner_only_key_and_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = dir.path().join("policy-authority.key");
        let mut out = Vec::new();

        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::CreatePolicyAuthorityKey {
                key: key_path.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();

        let metadata = std::fs::symlink_metadata(&key_path).unwrap();
        assert_eq!(metadata.len(), 32);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.policy-authority-key-created/v1"
        );
        assert_eq!(receipt["created"], true);
    }

    #[cfg(unix)]
    fn owner_only_dir(path: &std::path::Path) {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }

    /// Custody provisioning validates every ancestor's owner and mode, and the
    /// std temp root is structurally unsafe on some hosts (see
    /// `safe_ancestor_tempdir` in the protected-content runtime tests); anchor
    /// fixtures under the build target dir instead.
    #[cfg(unix)]
    fn safe_ancestor_tempdir() -> tempfile::TempDir {
        let root = option_env!("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../target")));
        let base = root.join("protected-content-config-test-fixtures");
        std::fs::create_dir_all(&base).unwrap();
        // Custody provisioning also rejects `..` path components, so hand it a
        // canonical base.
        tempfile::tempdir_in(base.canonicalize().unwrap()).unwrap()
    }

    #[cfg(unix)]
    fn test_runtime_issuer_hex() -> String {
        use elastos_runtime::signature::SigningKey;
        let key = SigningKey::from_bytes(&[0x42; 32]);
        format!("0x{}", hex::encode(key.verifying_key().to_bytes()))
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provision_custody_node_provisions_inactive_root_and_writes_descriptor() {
        use std::os::unix::fs::PermissionsExt;

        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);
        let descriptor_path = dir.path().join("node-a.descriptor.json");
        let mut out = Vec::new();

        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ProvisionCustodyNode {
                data_dir: data_dir.clone(),
                trusted_runtime_issuer: test_runtime_issuer_hex(),
                operator: "operator-a".to_string(),
                failure_domain: "failure-domain-a".to_string(),
                transport_peer_did: None,
                output: descriptor_path.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();

        let inactive_root = data_dir.join("protected-content/custody-provider/inactive");
        let metadata = std::fs::symlink_metadata(&inactive_root).unwrap();
        assert!(metadata.is_dir());
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);

        let descriptor_bytes = std::fs::read(&descriptor_path).unwrap();
        let descriptor: serde_json::Value = serde_json::from_slice(&descriptor_bytes).unwrap();
        assert_eq!(
            descriptor["schema"],
            "elastos.protected-content.custody-node-descriptor/v1"
        );
        let node_public_key = descriptor["node_public_key_hex"].as_str().unwrap();
        assert_eq!(node_public_key.len(), 2 + 64);
        assert!(node_public_key.starts_with("0x"));
        let custody_public_key = descriptor["node_custody_public_key_hex"].as_str().unwrap();
        assert_eq!(custody_public_key.len(), 2 + 1216 * 2);
        assert_eq!(descriptor["operator"], "operator-a");
        assert_eq!(descriptor["failure_domain"], "failure-domain-a");
        assert_eq!(descriptor["transport"]["kind"], "local");

        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.custody-node-provisioned/v1"
        );
        assert_eq!(receipt["node_public_key_hex"], node_public_key);
        assert_eq!(receipt["created"], true);
    }

    /// The Runtime validates owner-only modes from `protected-content` down,
    /// not on the data root itself; a 0755 data dir must stay provisionable.
    #[cfg(unix)]
    #[tokio::test]
    async fn provision_custody_node_accepts_a_group_readable_data_dir() {
        use std::os::unix::fs::PermissionsExt;

        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        std::fs::create_dir(&data_dir).unwrap();
        std::fs::set_permissions(&data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ProvisionCustodyNode {
                data_dir: data_dir.clone(),
                trusted_runtime_issuer: test_runtime_issuer_hex(),
                operator: "operator-a".to_string(),
                failure_domain: "failure-domain-a".to_string(),
                transport_peer_did: None,
                output: dir.path().join("node.descriptor.json"),
            },
            &mut out,
        )
        .await
        .unwrap();

        let protected_root = data_dir.join("protected-content");
        let metadata = std::fs::symlink_metadata(&protected_root).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn provision_custody_node_reprovision_keeps_the_same_node_identity() {
        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);
        let issuer = test_runtime_issuer_hex();

        let mut first_out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ProvisionCustodyNode {
                data_dir: data_dir.clone(),
                trusted_runtime_issuer: issuer.clone(),
                operator: "operator-a".to_string(),
                failure_domain: "failure-domain-a".to_string(),
                transport_peer_did: None,
                output: dir.path().join("first.descriptor.json"),
            },
            &mut first_out,
        )
        .await
        .unwrap();

        let mut second_out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ProvisionCustodyNode {
                data_dir: data_dir.clone(),
                trusted_runtime_issuer: issuer,
                operator: "operator-a".to_string(),
                failure_domain: "failure-domain-a".to_string(),
                transport_peer_did: None,
                output: dir.path().join("second.descriptor.json"),
            },
            &mut second_out,
        )
        .await
        .unwrap();

        let first: serde_json::Value = serde_json::from_slice(&first_out).unwrap();
        let second: serde_json::Value = serde_json::from_slice(&second_out).unwrap();
        assert_eq!(first["node_public_key_hex"], second["node_public_key_hex"]);
    }

    /// Provision three distinct custody hosts and return their descriptor paths.
    #[cfg(unix)]
    async fn provision_three_node_descriptors(
        dir: &std::path::Path,
        issuer_hex: &str,
        transports: [Option<String>; 3],
    ) -> Vec<PathBuf> {
        let mut descriptors = Vec::new();
        for (index, transport_peer_did) in transports.into_iter().enumerate() {
            let host_data_dir = dir.join(format!("host-{index}/data"));
            std::fs::create_dir_all(host_data_dir.parent().unwrap()).unwrap();
            owner_only_dir(&host_data_dir);
            let descriptor_path = dir.join(format!("node-{index}.descriptor.json"));
            let mut out = Vec::new();
            run_protected_content_config_command_with_writer(
                ProtectedContentConfigCommand::ProvisionCustodyNode {
                    data_dir: host_data_dir,
                    trusted_runtime_issuer: issuer_hex.to_string(),
                    operator: format!("operator-{index}"),
                    failure_domain: format!("failure-domain-{index}"),
                    transport_peer_did,
                    output: descriptor_path.clone(),
                },
                &mut out,
            )
            .await
            .unwrap();
            descriptors.push(descriptor_path);
        }
        descriptors
    }

    #[cfg(unix)]
    fn test_peer_did(seed: u8) -> String {
        use elastos_runtime::signature::SigningKey;
        crate::crypto::encode_did_key(&SigningKey::from_bytes(&[seed; 32]).verifying_key()).unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_custody_composition_output_passes_the_runtime_loader() {
        use elastos_runtime::provider::ProviderRegistry;
        use std::sync::Arc;

        let dir = safe_ancestor_tempdir();
        let runtime_data_dir = dir.path().join("runtime/data");
        std::fs::create_dir_all(runtime_data_dir.parent().unwrap()).unwrap();
        owner_only_dir(&runtime_data_dir);

        let authority_key_path = dir.path().join("policy-authority.key");
        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::CreatePolicyAuthorityKey {
                key: authority_key_path.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();

        let descriptors = provision_three_node_descriptors(
            dir.path(),
            &test_runtime_issuer_hex(),
            [None, Some(test_peer_did(0x51)), Some(test_peer_did(0x52))],
        )
        .await;

        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::GenerateCustodyComposition {
                authority_key: authority_key_path,
                nodes: descriptors,
                data_dir: runtime_data_dir.clone(),
                valid_days: 365,
            },
            &mut out,
        )
        .await
        .unwrap();

        // The loader's [_; 3] node array plus signature validation make this
        // the whole done-check: parse, canonical bytes, trust anchors, routes.
        crate::protected_content_runtime::load_runtime_custody_composition(
            &runtime_data_dir,
            Arc::new(ProviderRegistry::new()),
        )
        .unwrap()
        .expect("generated composition must load");

        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.custody-composition-generated/v1"
        );
        assert_eq!(receipt["created"], true);
        assert_eq!(
            receipt["node_public_key_hexes"].as_array().unwrap().len(),
            3
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_custody_composition_rejects_duplicate_operator_labels() {
        let dir = safe_ancestor_tempdir();
        let runtime_data_dir = dir.path().join("runtime/data");
        std::fs::create_dir_all(runtime_data_dir.parent().unwrap()).unwrap();
        owner_only_dir(&runtime_data_dir);

        let authority_key_path = dir.path().join("policy-authority.key");
        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::CreatePolicyAuthorityKey {
                key: authority_key_path.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();

        let descriptors = provision_three_node_descriptors(
            dir.path(),
            &test_runtime_issuer_hex(),
            [None, Some(test_peer_did(0x51)), Some(test_peer_did(0x52))],
        )
        .await;
        // Rewrite the third descriptor with the first one's operator label.
        let mut third: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&descriptors[2]).unwrap()).unwrap();
        third["operator"] = serde_json::Value::String("operator-0".to_string());
        std::fs::write(&descriptors[2], serde_json::to_vec(&third).unwrap()).unwrap();

        let mut out = Vec::new();
        let error = run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::GenerateCustodyComposition {
                authority_key: authority_key_path,
                nodes: descriptors,
                data_dir: runtime_data_dir.clone(),
                valid_days: 365,
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("distinct"), "{error}");
        assert!(!runtime_data_dir
            .join("protected-content/custody-composition.json")
            .exists());
    }

    #[cfg(unix)]
    async fn generate_test_composition(dir: &tempfile::TempDir) -> PathBuf {
        let runtime_data_dir = dir.path().join("runtime/data");
        std::fs::create_dir_all(runtime_data_dir.parent().unwrap()).unwrap();
        owner_only_dir(&runtime_data_dir);
        let authority_key_path = dir.path().join("policy-authority.key");
        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::CreatePolicyAuthorityKey {
                key: authority_key_path.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();
        let descriptors = provision_three_node_descriptors(
            dir.path(),
            &test_runtime_issuer_hex(),
            [None, Some(test_peer_did(0x51)), Some(test_peer_did(0x52))],
        )
        .await;
        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::GenerateCustodyComposition {
                authority_key: authority_key_path,
                nodes: descriptors,
                data_dir: runtime_data_dir.clone(),
                valid_days: 365,
            },
            &mut out,
        )
        .await
        .unwrap();
        runtime_data_dir
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verify_custody_composition_reports_the_installed_composition() {
        let dir = safe_ancestor_tempdir();
        let runtime_data_dir = generate_test_composition(&dir).await;

        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::VerifyCustodyComposition {
                data_dir: runtime_data_dir,
            },
            &mut out,
        )
        .await
        .unwrap();

        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.custody-composition-verified/v1"
        );
        assert_eq!(receipt["verified"], true);
        assert_eq!(
            receipt["node_public_key_hexes"].as_array().unwrap().len(),
            3
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn verify_custody_composition_fails_closed_when_the_composition_is_missing() {
        let dir = safe_ancestor_tempdir();
        let runtime_data_dir = dir.path().join("runtime/data");
        std::fs::create_dir_all(runtime_data_dir.parent().unwrap()).unwrap();
        owner_only_dir(&runtime_data_dir);

        let mut out = Vec::new();
        let error = run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::VerifyCustodyComposition {
                data_dir: runtime_data_dir,
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("missing"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn show_runtime_issuer_prints_the_derived_issuer_for_an_existing_device_key() {
        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);
        let device_key = elastos_identity::load_or_create_device_key(&data_dir).unwrap();
        let expected =
            crate::protected_content_runtime::derive_protected_content_runtime_issuer(&device_key)
                .unwrap();

        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ShowRuntimeIssuer {
                data_dir: data_dir.clone(),
            },
            &mut out,
        )
        .await
        .unwrap();

        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.runtime-issuer/v1"
        );
        assert_eq!(
            receipt["trusted_runtime_issuer"],
            format!("0x{}", hex::encode(expected.as_bytes()))
        );
    }

    #[cfg(unix)]
    fn chain_config_command(
        data_dir: &std::path::Path,
        evidence_rpc_urls: Vec<String>,
    ) -> ProtectedContentConfigCommand {
        ProtectedContentConfigCommand::GenerateChainConfig {
            data_dir: data_dir.to_path_buf(),
            id: "base-mainnet".to_string(),
            display_name: "Base".to_string(),
            chain_id: 8453,
            native_symbol: "ETH".to_string(),
            mainnet: true,
            rpc_url: "https://private-primary.example.invalid".to_string(),
            evidence_rpc_urls,
            rights_contract: "0x09dbe796f40eceffeaccf243c3d758c4c1d8d87d".to_string(),
            rights_selector: "0x54d42821".to_string(),
            authority_gateway_contract: "0x09dbe796f40eceffeaccf243c3d758c4c1d8d87d".to_string(),
            mint_ledger: "0x0000000000000000000000000000000000000022".to_string(),
            mint_pay_token: "0x0000000000000000000000000000000000000033".to_string(),
            mint_asset_created_emitter: "0x0000000000000000000000000000000000000044".to_string(),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_chain_config_installs_a_loadable_owner_only_config() {
        use std::os::unix::fs::PermissionsExt;

        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);

        let mut out = Vec::new();
        run_protected_content_config_command_with_writer(
            chain_config_command(
                &data_dir,
                vec![
                    "https://evidence-a.example.invalid".to_string(),
                    "https://evidence-b.example.invalid".to_string(),
                ],
            ),
            &mut out,
        )
        .await
        .unwrap();

        let loaded =
            crate::protected_content_runtime::load_runtime_protected_content_chain_provider_config(
                &data_dir,
            )
            .unwrap()
            .expect("generated chain config must load");
        let network = loaded.protected_content_network();
        assert_eq!(network["chain_id"], 8453);
        assert_eq!(network["rights_methods"][0]["selector"], "0x54d42821");
        assert_eq!(
            network["rights_methods"][0]["protected_content_policies"][0]["evidence_rpc_urls"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let path = data_dir.join("protected-content/chain-provider.json");
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

        let receipt: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            receipt["schema"],
            "elastos.protected-content.chain-provider-config-generated/v1"
        );
        assert_eq!(receipt["created"], true);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_chain_config_rejects_a_single_evidence_source() {
        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);

        let mut out = Vec::new();
        let error = run_protected_content_config_command_with_writer(
            chain_config_command(
                &data_dir,
                vec!["https://evidence-a.example.invalid".to_string()],
            ),
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("evidence"), "{error}");
        assert!(!data_dir
            .join("protected-content/chain-provider.json")
            .exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn generate_chain_config_rejects_duplicate_evidence_origins() {
        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);

        let mut out = Vec::new();
        let error = run_protected_content_config_command_with_writer(
            chain_config_command(
                &data_dir,
                vec![
                    "https://evidence-a.example.invalid/one".to_string(),
                    "https://evidence-a.example.invalid/two".to_string(),
                ],
            ),
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("origin"), "{error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn show_runtime_issuer_fails_closed_without_a_device_key() {
        let dir = safe_ancestor_tempdir();
        let data_dir = dir.path().join("data");
        owner_only_dir(&data_dir);

        let mut out = Vec::new();
        let error = run_protected_content_config_command_with_writer(
            ProtectedContentConfigCommand::ShowRuntimeIssuer { data_dir },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("device key"), "{error}");
    }
}
