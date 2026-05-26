use super::*;

pub(in crate::api::gateway) async fn wallet_connector_config(
    State(state): State<GatewayState>,
    Path(wallet_connector): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_wallet_connector_launch_context(
        &state.data_dir,
        &headers,
        wallet_connector.as_str(),
    ) {
        return system_error_response(err);
    }

    let mut payload = serde_json::json!({
        "schema": "elastos.wallet.connector.config/v1",
        "connector_id": wallet_connector.clone(),
        "label": wallet_connector_label(wallet_connector.as_str()),
        "evm_chains": wallet_connector_evm_chains(),
    });
    if wallet_connector == WALLET_WALLETCONNECT_CAPSULE_ID {
        match load_walletconnect_connector_config(&state.data_dir).and_then(|config| {
            validate_walletconnect_connector_config(&config)?;
            Ok(config)
        }) {
            Ok(config) => {
                payload["walletconnect"] = serde_json::json!({
                    "project_id": config.project_id,
                    "sdk_package": config.sdk_package,
                    "sdk_version": config.sdk_version,
                    "sdk_asset_path": "/apps/wallet-walletconnect/vendor/reown-appkit.js",
                    "sdk_sha256": config.sdk_sha256,
                });
            }
            Err(err) => return system_error_response(err),
        }
    }
    Json(payload).into_response()
}

pub(in crate::api::gateway) async fn wallet_connector_accounts(
    State(state): State<GatewayState>,
    Path(wallet_connector): Path<String>,
    headers: HeaderMap,
) -> Response {
    let context = match require_wallet_connector_launch_context(
        &state.data_dir,
        &headers,
        wallet_connector.as_str(),
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    let mut summary = system_wallet_accounts_summary(&state, &context.principal_id).await;
    summary.accounts.retain(|account| {
        !is_managed_wallet_proof_type(&account.proof_type)
            && account.connector_id.as_deref() == Some(wallet_connector.as_str())
    });
    summary.linked_count = summary.accounts.len();
    Json(summary).into_response()
}

pub(in crate::api::gateway) async fn wallet_connector_approvals(
    State(state): State<GatewayState>,
    Path(wallet_connector): Path<String>,
    headers: HeaderMap,
) -> Response {
    let context = match require_wallet_connector_launch_context(
        &state.data_dir,
        &headers,
        wallet_connector.as_str(),
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    let mut summary = system_wallet_approvals_summary(&state, &context.principal_id, false).await;
    summary.approval_requests.retain(|request| {
        !is_managed_wallet_proof_type(&request.proof_type)
            && request.connector_id.as_deref() == Some(wallet_connector.as_str())
    });
    summary.pending_count = summary.approval_requests.len();
    Json(summary).into_response()
}

pub(in crate::api::gateway) async fn wallet_connector_approval_approve(
    State(state): State<GatewayState>,
    Path((wallet_connector, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<WalletApprovalApproveRequest>,
) -> Response {
    let context = match require_wallet_connector_launch_context(
        &state.data_dir,
        &headers,
        wallet_connector.as_str(),
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    let connector_label = wallet_connector_label(wallet_connector.as_str());
    let reason = input
        .reason
        .unwrap_or_else(|| format!("Approved in {connector_label} connector"));
    match approve_external_wallet_request(
        &state,
        &state.data_dir,
        &context.principal_id,
        &context.session_id,
        &request_id,
        &reason,
        wallet_connector.as_str(),
    )
    .await
    {
        Ok(outcome) => {
            let mut summary =
                system_wallet_approvals_summary(&state, &context.principal_id, false).await;
            summary.note = Some(format!("Approved. Continue in {connector_label}."));
            summary.handoff = outcome.handoff;
            Json(summary).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_connector_approval_complete(
    State(state): State<GatewayState>,
    Path((wallet_connector, request_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<WalletApprovalCompleteRequest>,
) -> Response {
    let context = match require_wallet_connector_launch_context(
        &state.data_dir,
        &headers,
        wallet_connector.as_str(),
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    let connector_label = wallet_connector_label(wallet_connector.as_str());
    match complete_external_wallet_approval(
        &state,
        &context,
        &request_id,
        input,
        wallet_connector.as_str(),
        &format!("External wallet signature completed through {connector_label} approval"),
    )
    .await
    {
        Ok(mut summary) => {
            summary.note = Some(format!("Signed by {connector_label}."));
            Json(summary).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) fn require_wallet_connector_launch_context(
    data_dir: &FsPath,
    headers: &HeaderMap,
    connector_id: &str,
) -> anyhow::Result<HomeLaunchTokenContext> {
    if !is_wallet_connector_capsule_id(connector_id) {
        anyhow::bail!("unknown wallet connector capsule");
    }
    ensure_wallet_connector_configured(data_dir, connector_id)?;
    require_home_launch_token_context(data_dir, headers, connector_id)
}

pub(in crate::api::gateway) fn require_wallet_app_launch_context(
    data_dir: &FsPath,
    headers: &HeaderMap,
) -> anyhow::Result<HomeLaunchTokenContext> {
    require_home_launch_token_context(data_dir, headers, WALLET_CAPSULE_ID)
}

pub(crate) fn ensure_wallet_connector_configured(
    data_dir: &FsPath,
    connector_id: &str,
) -> anyhow::Result<()> {
    if connector_id != WALLET_WALLETCONNECT_CAPSULE_ID {
        return Ok(());
    }
    let config = load_walletconnect_connector_config(data_dir)?;
    validate_walletconnect_connector_config(&config)?;

    let sdk_path = data_dir.join(WALLETCONNECT_SDK_PATH);
    let bytes = std::fs::read(&sdk_path).map_err(|err| {
        anyhow::anyhow!(
            "WalletConnect connector is not configured: pinned SDK asset missing ({err})"
        )
    })?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if !digest.eq_ignore_ascii_case(config.sdk_sha256.trim()) {
        anyhow::bail!("WalletConnect connector is not configured: pinned SDK hash mismatch");
    }
    Ok(())
}

pub(in crate::api::gateway) fn load_walletconnect_connector_config(
    data_dir: &FsPath,
) -> anyhow::Result<WalletConnectConnectorConfig> {
    let path = data_dir.join(WALLETCONNECT_CONFIG_PATH);
    let bytes = std::fs::read(&path).map_err(|err| {
        anyhow::anyhow!("WalletConnect connector is not configured: pinned config missing ({err})")
    })?;
    serde_json::from_slice(&bytes).map_err(|err| {
        anyhow::anyhow!("WalletConnect connector is not configured: invalid pinned config ({err})")
    })
}

pub(in crate::api::gateway) fn validate_walletconnect_connector_config(
    config: &WalletConnectConnectorConfig,
) -> anyhow::Result<()> {
    if config.schema != WALLETCONNECT_CONFIG_SCHEMA {
        anyhow::bail!("WalletConnect connector is not configured: unsupported config schema");
    }
    let project_id = config.project_id.trim();
    if project_id.len() < 8
        || project_id.len() > 128
        || !project_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        anyhow::bail!("WalletConnect connector is not configured: invalid project id");
    }
    if config.sdk_package != WALLETCONNECT_SDK_PACKAGE {
        anyhow::bail!("WalletConnect connector is not configured: unsupported SDK package");
    }
    let version = config.sdk_version.trim();
    if version.is_empty()
        || version.starts_with('^')
        || version.starts_with('~')
        || version.starts_with('*')
        || version.contains(char::is_whitespace)
    {
        anyhow::bail!("WalletConnect connector is not configured: SDK version must be pinned");
    }
    let digest = config.sdk_sha256.trim();
    if digest.len() != 64 || !digest.chars().all(|ch| ch.is_ascii_hexdigit()) {
        anyhow::bail!("WalletConnect connector is not configured: invalid SDK hash");
    }
    Ok(())
}
