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
    /// Save a new signing key and advance the token epoch; restart Runtime to apply the key
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
            println!("New signing key saved to disk.");
            println!("  New public key: {}", hex::encode(new_pub));
            println!("  Durable epoch advanced: tokens from earlier epochs are rejected.");
            println!("  Reason: {}", reason);
            println!();
            println!("Restart required: restart every Runtime that uses this data directory to apply the new key.");
            println!("Running Runtime processes retain their loaded signing key until restart.");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emergency_rotation_cli_reports_restart_requirement() {
        const CHILD_DIR: &str = "ELASTOS_EMERGENCY_ROTATION_TEST_DIR";
        if let Some(directory) = std::env::var_os(CHILD_DIR) {
            use clap::Parser as _;

            let cli = crate::Cli::try_parse_from([
                "elastos",
                "emergency",
                "rotate",
                "--reason",
                "isolated command fixture",
            ])
            .unwrap();
            let Some(crate::Commands::Emergency(command)) = cli.command else {
                panic!("the CLI must dispatch emergency rotation");
            };
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            runtime
                .block_on(run_emergency_in(std::path::Path::new(&directory), command))
                .unwrap();
            return;
        }

        let data_dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let store_path = data_dir.path().join("capability_store");
        let store = runtime
            .block_on(capability::CapabilityStore::with_persistence(&store_path))
            .unwrap();
        assert_eq!(store.advance_epoch().unwrap(), 1);

        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "security_cmd::tests::emergency_rotation_cli_reports_restart_requirement",
                "--nocapture",
            ])
            .env(CHILD_DIR, data_dir.path())
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "rotation command failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("running 1 test"), "{output}");
        assert!(
            output.contains("New signing key saved to disk."),
            "{output}"
        );
        assert!(
            output.contains("Durable epoch advanced: tokens from earlier epochs are rejected."),
            "{output}"
        );
        assert!(
            output.contains("Restart required: restart every Runtime that uses this data directory to apply the new key."),
            "{output}"
        );
        assert!(
            output.contains(
                "Running Runtime processes retain their loaded signing key until restart."
            ),
            "{output}"
        );
        assert!(!output.contains("all prior tokens invalidated"), "{output}");
        assert_eq!(
            runtime
                .block_on(capability::CapabilityStore::with_persistence(&store_path))
                .unwrap()
                .current_epoch(),
            2
        );
        assert_eq!(
            std::fs::read(data_dir.path().join("signing_key"))
                .unwrap()
                .len(),
            32
        );
    }

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
        for missing in [true, false] {
            let data_dir = tempfile::tempdir().unwrap();
            let store_path = data_dir.path().join("capability_store");
            let store = capability::CapabilityStore::with_persistence(&store_path)
                .await
                .unwrap();
            assert_eq!(store.advance_epoch().unwrap(), 1);
            let epoch_path = store_path.join("epoch");
            if missing {
                std::fs::remove_file(&epoch_path).unwrap();
            } else {
                std::fs::write(&epoch_path, "invalid").unwrap();
            }
            let error = run_emergency_in(
                data_dir.path(),
                EmergencyCommand::Rotate {
                    reason: "isolated command fixture".into(),
                },
            )
            .await
            .expect_err("the command must refuse unreadable durable state")
            .to_string();
            assert!(
                error.contains("Capability store startup refused"),
                "{error}"
            );
            assert!(error.contains(&epoch_path.display().to_string()), "{error}");
            assert!(error.contains("latest committed epoch"), "{error}");
            assert!(!data_dir.path().join("signing_key").exists());
            if missing {
                assert!(!epoch_path.exists());
            } else {
                assert_eq!(std::fs::read_to_string(&epoch_path).unwrap(), "invalid");
            }
        }
    }
}
