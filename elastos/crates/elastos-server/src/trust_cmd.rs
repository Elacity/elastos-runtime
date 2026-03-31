use clap::Subcommand;
use std::io::Read as _;
use std::path::PathBuf;

use elastos_runtime::signature::{
    generate_keypair, hash_content, sign_capsule, SignatureVerifier, SigningKey,
};

#[derive(Subcommand)]
pub enum KeysCommand {
    /// Generate a new signing keypair
    Generate {
        /// Output directory for keys (creates private.key and public.key)
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
    /// Print the stable P2P node ID (derived from device key)
    NodeId,
}

pub fn run_keys(keys_cmd: KeysCommand) -> anyhow::Result<()> {
    match keys_cmd {
        KeysCommand::Generate { output } => {
            let (signing_key, verifying_key) = generate_keypair();

            std::fs::create_dir_all(&output)?;

            let private_path = output.join("private.key");
            let private_hex = hex::encode(signing_key.to_bytes());
            std::fs::write(&private_path, &private_hex)?;

            let public_path = output.join("public.key");
            let public_hex = hex::encode(verifying_key.as_bytes());
            std::fs::write(&public_path, &public_hex)?;

            println!("Generated new keypair:");
            println!("  Private key: {}", private_path.display());
            println!("  Public key:  {}", public_path.display());
            println!("\nPublic key (for sharing): {}", public_hex);
        }
        KeysCommand::NodeId => {
            let data_dir = elastos_server::sources::default_data_dir();
            let (_signing_key, did) = elastos_identity::load_or_create_did(&data_dir)?;
            println!("{}", did);
        }
    }

    Ok(())
}

pub fn run_sign(path: PathBuf, key: PathBuf) -> anyhow::Result<()> {
    let key_hex = std::fs::read_to_string(&key)?;
    let key_bytes =
        hex::decode(key_hex.trim()).map_err(|e| anyhow::anyhow!("Invalid key format: {}", e))?;
    let key_array: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("Key must be 32 bytes"))?;
    let signing_key = SigningKey::from_bytes(&key_array);

    let manifest_path = path.join("capsule.json");
    let manifest_data = std::fs::read_to_string(&manifest_path)?;
    let mut manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;

    let entrypoint_path = path.join(&manifest.entrypoint);
    let content = std::fs::read(&entrypoint_path)?;
    let content_hash = hash_content(&content);

    sign_capsule(&signing_key, &mut manifest, &content_hash)?;

    let updated_manifest = serde_json::to_string_pretty(&manifest)?;
    std::fs::write(&manifest_path, &updated_manifest)?;

    println!("Signed capsule '{}' at {}", manifest.name, path.display());
    println!("Signature added to capsule.json");

    Ok(())
}

pub async fn run_verify(
    path: Option<PathBuf>,
    public_key: Option<PathBuf>,
    cid: Option<String>,
    provenance: Option<String>,
) -> anyhow::Result<()> {
    if let Some(cid_str) = cid {
        run_verify_provenance(cid_str, provenance).await?;
    } else {
        let path = path.expect("path is required by clap when --cid is absent");
        let public_key = public_key.expect("public_key is required by clap when --cid is absent");
        run_verify_capsule(path, public_key)?;
    }

    Ok(())
}

pub async fn run_attest(
    cid: String,
    key: Option<PathBuf>,
    content_digest: Option<String>,
) -> anyhow::Result<()> {
    let ipfs = crate::get_ipfs_bridge().await?;

    if !elastos_server::shares::is_valid_cid(&cid) {
        anyhow::bail!("Invalid CID: {}", cid);
    }

    let signing_key = load_signing_key_or_default(key)?;
    let digest = match content_digest {
        Some(d) => d,
        None => {
            println!("Fetching _share.json from {}...", cid);
            let share_bytes = ipfs.cat_with_path(&cid, "_share.json").await?;
            let share_meta: serde_json::Value = serde_json::from_slice(&share_bytes)?;
            share_meta["content_digest"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("No content_digest in _share.json"))?
                .to_string()
        }
    };

    let prov_bytes = elastos_server::shares::create_provenance(&cid, &digest, &signing_key)?;
    let prov_cid = ipfs.add_bytes(&prov_bytes, "provenance.json").await?;
    let did = elastos_server::crypto::encode_did_key(&signing_key.verifying_key());
    println!("Provenance attestation published!");
    println!("  Subject CID:    {}", cid);
    println!("  Provenance CID: {}", prov_cid);
    println!("  Builder DID:    {}", did);

    let mut catalog = elastos_server::shares::load_share_catalog()?;
    let mut updated = false;
    for ch in catalog.channels.values_mut() {
        for entry in &mut ch.history {
            if entry.cid == cid && entry.provenance_cid.is_none() {
                entry.provenance_cid = Some(prov_cid.clone());
                updated = true;
            }
        }
    }
    if updated {
        elastos_server::shares::save_share_catalog(&catalog)?;
        println!("  Local catalog updated.");
    }

    Ok(())
}

pub fn run_sign_payload(domain: String, key: Option<PathBuf>) -> anyhow::Result<()> {
    let mut payload = Vec::new();
    std::io::stdin().read_to_end(&mut payload)?;

    let signing_key = load_signing_key_or_default(key)?;
    let (sig_hex, did) =
        elastos_server::crypto::domain_separated_sign(&signing_key, &domain, &payload);
    println!(
        "{}",
        serde_json::json!({
            "signature": sig_hex,
            "signer_did": did,
        })
    );

    Ok(())
}

fn run_verify_capsule(path: PathBuf, public_key: PathBuf) -> anyhow::Result<()> {
    let key_hex = std::fs::read_to_string(&public_key)?;
    let mut verifier = SignatureVerifier::new();
    verifier.add_trusted_key_hex(key_hex.trim())?;

    let manifest_path = path.join("capsule.json");
    let manifest_data = std::fs::read_to_string(&manifest_path)?;
    let manifest: elastos_common::CapsuleManifest = serde_json::from_str(&manifest_data)?;
    manifest
        .validate()
        .map_err(|e| anyhow::anyhow!("Invalid manifest: {}", e))?;

    if manifest.signature.is_none() {
        println!("Capsule is NOT signed");
        std::process::exit(1);
    }

    let entrypoint_path = path.join(&manifest.entrypoint);
    let content = std::fs::read(&entrypoint_path)?;
    let content_hash = hash_content(&content);

    match verifier.verify_capsule(&manifest, &content_hash) {
        Ok(true) => {
            println!("Signature VALID for capsule '{}'", manifest.name);
            Ok(())
        }
        Ok(false) => {
            println!("Signature INVALID - key mismatch or tampered content");
            std::process::exit(1);
        }
        Err(e) => {
            println!("Verification error: {}", e);
            std::process::exit(1);
        }
    }
}

async fn run_verify_provenance(cid_str: String, provenance: Option<String>) -> anyhow::Result<()> {
    if !elastos_server::shares::is_valid_cid(&cid_str) {
        anyhow::bail!("Invalid CID: {}", cid_str);
    }

    let prov_cid = match provenance {
        Some(p) => {
            if !elastos_server::shares::is_valid_cid(&p) {
                anyhow::bail!("Invalid provenance CID: {}", p);
            }
            p
        }
        None => find_provenance_cid_for_subject(&cid_str)?,
    };

    println!("Fetching provenance {}...", prov_cid);
    let ipfs = crate::get_ipfs_bridge().await?;
    let prov_bytes = ipfs.cat(&prov_cid).await?;
    match elastos_server::shares::verify_provenance(&prov_bytes, &cid_str) {
        Ok(prov) => {
            println!("Provenance VALID");
            println!("  Subject CID:    {}", prov.payload.subject_cid);
            println!("  Builder DID:    {}", prov.payload.builder_did);
            println!("  Content digest: {}", prov.payload.content_digest);
            println!("  Built at:       {}", prov.payload.built_at);
            println!("  Tool:           {}", prov.payload.tool_version);
            Ok(())
        }
        Err(e) => {
            println!("Provenance INVALID: {}", e);
            std::process::exit(1);
        }
    }
}

fn find_provenance_cid_for_subject(cid_str: &str) -> anyhow::Result<String> {
    let catalog = elastos_server::shares::load_share_catalog()?;
    for channel in catalog.channels.values() {
        for entry in &channel.history {
            if entry.cid == cid_str {
                if let Some(provenance_cid) = entry.provenance_cid.clone() {
                    return Ok(provenance_cid);
                }
            }
        }
    }

    anyhow::bail!(
        "No provenance found for {}. Use --provenance <cid> to provide one.",
        cid_str
    );
}

fn load_signing_key_or_default(key: Option<PathBuf>) -> anyhow::Result<SigningKey> {
    match key {
        Some(key_path) => {
            let hex_str = std::fs::read_to_string(&key_path).map_err(|e| {
                anyhow::anyhow!("Cannot read key file {}: {}", key_path.display(), e)
            })?;
            let bytes = hex::decode(hex_str.trim())
                .map_err(|e| anyhow::anyhow!("Invalid hex in key file: {}", e))?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| anyhow::anyhow!("Invalid Ed25519 key (expected 32 bytes)"))?;
            Ok(SigningKey::from_bytes(&arr))
        }
        None => Ok(elastos_server::shares::load_or_create_share_key()?),
    }
}
