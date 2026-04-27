use crate::runtime_control::{
    dispatch_via_existing_runtime, read_operator_runtime_coords, runtime_coord_path,
    OPERATOR_RUNTIME_REQUIRED_MESSAGE,
};
use crate::sources::{default_data_dir, OwnershipRepairGuard};

/// Forward a command to the shell via the supervisor path.
///
/// This is an operator-runtime command — it requires `elastos serve` to be
/// running. It does NOT auto-start a managed runtime. If the runtime is not
/// running, it fails fast with guidance.
pub async fn forward_to_shell(command: serde_json::Value) -> anyhow::Result<()> {
    let data_dir = default_data_dir();
    let _ownership_guard = OwnershipRepairGuard::new(data_dir.clone());
    let coords_path = runtime_coord_path(&data_dir);

    if let Some(coords) = read_operator_runtime_coords(&coords_path).await {
        return dispatch_via_existing_runtime(&coords, command).await;
    }

    // No running runtime — fail clearly. This is an operator-runtime command.
    anyhow::bail!(OPERATOR_RUNTIME_REQUIRED_MESSAGE);
}
