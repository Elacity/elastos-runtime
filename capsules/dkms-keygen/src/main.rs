//! `dkms-keygen` — operator-console tool for the dKMS deployment kit
//! (`docs/convergence/dkms-deploy/`).
//!
//! Mints the two NON-NODE identities the deployment references, using the runtime's OWN crypto as
//! the single source of truth: `ddrm_envelope::seal::mldsa_seal_keypair` (ML-DSA-65 / FIPS 204,
//! deterministic from a 32-byte seed). An emitted verifying key is therefore byte-identical to
//! what a node verifies and what `key-provider` re-derives — there is NO parallel keygen.
//!
//!   * `operator` — the lifecycle authority. Its seed (the signing key) stays on the operator
//!     console; its public VK is pinned into every node as `DKMS_AUTHORITY_OPERATOR_VK`. It
//!     authorizes rotate/revoke/reconfigure/DKG — i.e. exactly the membership-rotation lifecycle a
//!     yearly-elected DAO council would drive.
//!   * `caller`   — the runtime's allow-listed identity. Its seed becomes the runtime's
//!     `dkms_caller_seed_b64`; its public VK goes on every node's `DKMS_AUTHORITY_ALLOWED_CALLERS`.
//!
//! No dKMS master seed is ever produced here — those are born + held on the nodes.

use base64::Engine as _;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "dkms-keygen", about = "Mint dKMS operator/caller identities (ML-DSA-65)")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum)]
enum Role {
    /// Lifecycle authority — VK pinned as DKMS_AUTHORITY_OPERATOR_VK on every node.
    Operator,
    /// Runtime caller — seed becomes dkms_caller_seed_b64; VK goes on every node's allow-list.
    Caller,
}

impl Role {
    fn label(self) -> &'static str {
        match self {
            Role::Operator => "operator",
            Role::Caller => "caller",
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Mint a dKMS identity for an `operator` or runtime `caller`.
    Keygen {
        /// Which identity to mint.
        #[arg(long, value_enum)]
        role: Role,
        /// Optional directory to ALSO write `<role>.seed` (0600, SECRET) + `<role>.vk` (0644, public).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Re-derive the PUBLIC verifying key from a 32-byte seed (base64). Proves determinism: the
    /// SAME seed always yields the SAME VK a node's allow-list will accept (recovery + self-test).
    DeriveVk {
        /// The base64-encoded 32-byte seed.
        #[arg(long)]
        seed_b64: String,
    },
}

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

/// Derive the ML-DSA-65 verifying key from a 32-byte seed — the single source of truth.
fn derive(seed: [u8; 32]) -> Vec<u8> {
    let (_signer, vk) = ddrm_envelope::seal::mldsa_seal_keypair(seed);
    vk
}

fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Keygen { role, out } => keygen(role, out),
        Command::DeriveVk { seed_b64 } => {
            let seed = decode_seed(&seed_b64)?;
            println!("{}", b64().encode(derive(seed)));
            Ok(())
        }
    }
}

fn keygen(role: Role, out: Option<PathBuf>) -> anyhow::Result<()> {
    let seed = ddrm_envelope::random_seed();
    let vk = derive(seed);

    // SELF-TEST: a node accepts a caller/operator iff the VK is the deterministic image of the
    // seed (the runtime re-derives its caller VK from dkms_caller_seed_b64 on every launch). If a
    // fresh derivation from the same seed did not reproduce the VK, the allow-list would never
    // match — fail loudly rather than ship a broken identity.
    anyhow::ensure!(
        derive(seed) == vk,
        "keygen self-test FAILED: re-deriving the VK from the seed did not reproduce it"
    );

    let seed_b64 = b64().encode(seed);
    let vk_b64 = b64().encode(&vk);

    if let Some(dir) = &out {
        std::fs::create_dir_all(dir)?;
        let seed_path = dir.join(format!("{}.seed", role.label()));
        let vk_path = dir.join(format!("{}.vk", role.label()));
        std::fs::write(&seed_path, &seed_b64)?;
        std::fs::write(&vk_path, &vk_b64)?;
        set_secret_mode(&seed_path)?;
        eprintln!(
            "wrote {} (SECRET, 0600) and {}",
            seed_path.display(),
            vk_path.display()
        );
    }

    println!("# dKMS {} identity (ML-DSA-65)", role.label());
    println!("seed_b64: {seed_b64}");
    println!("vk_b64:   {vk_b64}");
    println!();
    match role {
        Role::Operator => {
            println!("Next steps:");
            println!("  • KEEP seed_b64 SECRET on the operator console (it is the operator signing key).");
            println!("  • Pin vk_b64 on EVERY node:  DKMS_AUTHORITY_OPERATOR_VK=<vk_b64>");
        }
        Role::Caller => {
            println!("Next steps:");
            println!("  • Put seed_b64 in the runtime key-provider init config:  dkms_caller_seed_b64=<seed_b64>");
            println!("  • Allow-list vk_b64 on EVERY node:  DKMS_AUTHORITY_ALLOWED_CALLERS=<vk_b64>");
        }
    }
    println!();
    println!("Verify any time:  dkms-keygen derive-vk --seed-b64 <seed_b64>   (must print vk_b64)");
    Ok(())
}

fn decode_seed(seed_b64: &str) -> anyhow::Result<[u8; 32]> {
    let bytes = b64()
        .decode(seed_b64.trim())
        .map_err(|e| anyhow::anyhow!("seed is not valid base64: {e}"))?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("seed must decode to exactly 32 bytes"))
}

#[cfg(unix)]
fn set_secret_mode(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_secret_mode(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The VK is a DETERMINISTIC function of the seed — the property the node allow-list and the
    /// runtime caller re-derivation both rely on. Same seed in → same VK out, every time.
    #[test]
    fn vk_is_deterministic_from_the_seed() {
        let seed = [7u8; 32];
        assert_eq!(derive(seed), derive(seed));
        assert!(!derive(seed).is_empty());
    }

    /// `derive-vk` on an emitted seed reproduces the emitted VK — the exact self-test the operator
    /// runs to confirm a caller seed will be accepted by a node provisioned with the matching VK.
    #[test]
    fn derive_vk_round_trips_through_base64() {
        let seed = ddrm_envelope::random_seed();
        let vk_b64 = b64().encode(derive(seed));
        let seed_b64 = b64().encode(seed);
        let reseed = decode_seed(&seed_b64).unwrap();
        assert_eq!(b64().encode(derive(reseed)), vk_b64);
    }

    /// Distinct seeds produce distinct identities (no accidental collisions across roles).
    #[test]
    fn distinct_seeds_distinct_vks() {
        assert_ne!(derive([1u8; 32]), derive([2u8; 32]));
    }
}
