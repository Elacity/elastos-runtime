use clap::Subcommand;
use std::sync::Arc;

use elastos_runtime::{capability, primitives};

#[derive(Subcommand)]
pub enum EmergencyCommand {
    /// Rotate signing key and invalidate all sessions/tokens
    Rotate {
        /// Reason for the emergency rotation
        #[arg(long, default_value = "key compromise")]
        reason: String,
    },
}

pub fn run_emergency(cmd: EmergencyCommand) -> anyhow::Result<()> {
    match cmd {
        EmergencyCommand::Rotate { reason } => {
            let data_dir = elastos_server::sources::default_data_dir();
            elastos_server::data_root::require_private_data_root(&data_dir)?;

            let store = Arc::new(capability::CapabilityStore::new());
            let audit_log = Arc::new(primitives::audit::AuditLog::new());
            let metrics = Arc::new(primitives::metrics::MetricsManager::new());
            let mut mgr = capability::CapabilityManager::load_or_generate(
                &data_dir, store, audit_log, metrics,
            );

            let new_pub = mgr.rotate_signing_key(&data_dir, &reason);
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
