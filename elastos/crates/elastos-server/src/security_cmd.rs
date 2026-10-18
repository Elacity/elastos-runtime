use clap::Subcommand;
use std::sync::Arc;

use elastos_runtime::{capability, primitives};

#[derive(Subcommand)]
pub enum TlsCommand {
    /// Show CA certificate path and trust instructions
    Trust,
    /// Regenerate leaf certificate (e.g., after IP change)
    Regen,
}

#[derive(Subcommand)]
pub enum EmergencyCommand {
    /// Rotate signing key and invalidate all sessions/tokens
    Rotate {
        /// Reason for the emergency rotation
        #[arg(long, default_value = "key compromise")]
        reason: String,
    },
}

pub fn run_tls(tls_cmd: TlsCommand) -> anyhow::Result<()> {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp/elastos"))
        .join("elastos");
    match tls_cmd {
        TlsCommand::Trust => {
            elastos_tls::print_trust_instructions(&data_dir);
        }
        TlsCommand::Regen => {
            elastos_tls::regenerate_leaf(&data_dir)?;
        }
    }

    Ok(())
}

pub async fn run_emergency(cmd: EmergencyCommand) -> anyhow::Result<()> {
    let data_dir = elastos_server::sources::default_data_dir();
    run_emergency_in(&data_dir, cmd).await
}

async fn run_emergency_in(data_dir: &std::path::Path, cmd: EmergencyCommand) -> anyhow::Result<()> {
    match cmd {
        EmergencyCommand::Rotate { reason } => {
            std::fs::create_dir_all(data_dir)?;
            let store = Arc::new(
                capability::CapabilityStore::with_persistence(data_dir.join("capability_store"))
                    .await?,
            );
            let audit_log = Arc::new(primitives::audit::AuditLog::new());
            let metrics = Arc::new(primitives::metrics::MetricsManager::new());
            let mut mgr = capability::CapabilityManager::load_or_generate(
                data_dir, store, audit_log, metrics,
            );

            let new_pub = mgr.rotate_signing_key(data_dir, &reason)?;
            println!("Signing key rotated.");
            println!("  New public key: {}", hex::encode(new_pub));
            println!("  Epoch advanced: all prior tokens invalidated.");
            println!("  Reason: {}", reason);
            println!();
            println!("Restart the runtime to apply the new key.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn emergency_rotation_uses_the_commands_persistent_store() {
        let data_dir = tempfile::tempdir().unwrap();
        let store_path = data_dir.path().join("capability_store");
        let store = capability::CapabilityStore::with_persistence(&store_path)
            .await
            .unwrap();
        for _ in 0..7 {
            store.advance_epoch().unwrap();
        }
        for expected in [8, 9] {
            run_emergency_in(
                data_dir.path(),
                EmergencyCommand::Rotate {
                    reason: "isolated command fixture".into(),
                },
            )
            .await
            .unwrap();
            let reloaded = capability::CapabilityStore::with_persistence(&store_path)
                .await
                .unwrap();
            assert_eq!(
                reloaded.current_epoch(),
                expected,
                "the command must advance the durable epoch"
            );
            assert!(data_dir.path().join("signing_key").is_file());
        }
        // A separately loaded, older store also preserves the command result.
        store.persist().await.unwrap();
        let reloaded = capability::CapabilityStore::with_persistence(&store_path)
            .await
            .unwrap();
        assert_eq!(reloaded.current_epoch(), 9);
    }

    #[tokio::test]
    async fn emergency_rotation_refuses_unreadable_state_before_creating_a_key() {
        let data_dir = tempfile::tempdir().unwrap();
        let store_path = data_dir.path().join("capability_store");
        std::fs::create_dir(&store_path).unwrap();
        std::fs::write(store_path.join("epoch"), "invalid").unwrap();
        assert!(
            run_emergency_in(
                data_dir.path(),
                EmergencyCommand::Rotate {
                    reason: "isolated command fixture".into(),
                },
            )
            .await
            .is_err(),
            "the command must refuse malformed durable state"
        );
        assert!(!data_dir.path().join("signing_key").exists());
        assert_eq!(
            std::fs::read_to_string(store_path.join("epoch")).unwrap(),
            "invalid"
        );
    }
}
