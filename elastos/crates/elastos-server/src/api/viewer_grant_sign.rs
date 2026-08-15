//! Connector-brokered delegation-sign approval endpoints (ESP connector delegation signing,
//! Task 3).
//!
//! `/api/viewers/prepare-grant` (`viewer_open::prepare_owned_grant`, Task 2) hands the ESP shell a
//! `delegation_canonical` string plus the principal's `default_connector_id`. The shell must obtain
//! a wallet `personal_sign` over that EXACT string through the user's default wallet CONNECTOR
//! capsule (which has its own injected wallet) — never via `window.ethereum` in the shell itself.
//! These two endpoints mint that signature request and report its result:
//!
//!   POST /api/viewers/prepare-grant/sign        -> mints a connector `personal_sign` approval,
//!                                                   returns `{ request_id, connector_id }`.
//!   GET  /api/viewers/prepare-grant/sign/:id     -> reports `{ status, delegation_sig_hex? }` by
//!                                                   reading the SAME approval store the connector
//!                                                   completes into.
//!
//! Both the mint and the read-back reuse the SAME wallet-provider `RequestApproval` /
//! `ListApprovals` dispatch the Browser wallet bridge uses for its own connector `personal_sign`
//! approvals (`gateway_browser_wallet::create_browser_wallet_signature_request` /
//! `browser_wallet_approval_status`) — `create_connector_personal_sign_approval` is the exact same
//! mint core, parameterized here with `resource_action = "ddrm_delegation_sign"` instead of
//! `"browser_personal_sign"`, so an approval this endpoint mints and one the Browser bridge mints
//! can never drift on shape. The gate is the SAME owned-open launch-token authority
//! `viewer_open::prepare_owned_grant` uses (`[home, home-gui, home-cli]`), not a Browser-app-only
//! actor — this seam is reached from the ESP shell (Home/Home-GUI/Home-CLI), never from a Browser
//! page.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use super::gateway::{
    create_connector_personal_sign_approval, require_owned_open_token_launch,
    runtime_wallet_authority, runtime_wallet_data, system_wallet_accounts_summary,
    validate_connector_and_resolve_account, ConnectorPersonalSignApprovalInput, GatewayState,
    HomeLaunchTokenContext, RuntimeWalletAuthority,
};

/// The approval `intent` / resource-URI action segment for a dDRM delegation signature — see
/// `ConnectorPersonalSignApprovalInput::resource_action`. Distinguishes this endpoint's approvals
/// from the Browser wallet bridge's `browser_personal_sign` / `browser_typed_data_sign` ones in the
/// SAME per-principal approval store.
const DDRM_DELEGATION_SIGN_RESOURCE_ACTION: &str = "ddrm_delegation_sign";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrepareGrantSignRequest {
    uri: String,
    delegation_canonical: String,
    owner_address: String,
    kid: String,
    connector_id: String,
}

/// POST /api/viewers/prepare-grant/sign — mint a connector `personal_sign` approval whose message
/// is EXACTLY `delegation_canonical`, so the ESP shell can route it to the principal's default
/// wallet connector instead of asking `window.ethereum` for a signature itself.
pub async fn create_delegation_sign_request(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(req): Json<PrepareGrantSignRequest>,
) -> Response {
    // Identical actor allowlist to `viewer_open::prepare_owned_grant` — this does NOT admit any
    // caller the owned-open context-only gate would have rejected. The FULL launch (not just the
    // context) is needed because minting a connector approval requires a `RuntimeWalletAuthority`,
    // which can only be minted from a whole `RequiredHomeLaunchToken`.
    let required_launch = match require_owned_open_token_launch(&state.data_dir, &headers) {
        Ok(required) => required,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let context = required_launch.context.clone();

    if req.uri.trim().is_empty()
        || req.owner_address.trim().is_empty()
        || req.kid.trim().is_empty()
        || req.connector_id.trim().is_empty()
    {
        return (
            StatusCode::BAD_REQUEST,
            "prepare-grant sign request is missing a required field",
        )
            .into_response();
    }
    if req.delegation_canonical.is_empty()
        || req.delegation_canonical.len() > 8 * 1024
        || req.delegation_canonical.chars().any(char::is_control)
    {
        return (StatusCode::BAD_REQUEST, "delegation_canonical is invalid").into_response();
    }

    let authority = match runtime_wallet_authority(&required_launch) {
        Ok(authority) => authority,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };

    let summary = system_wallet_accounts_summary(&state, &authority).await;
    let (account_id, chain_namespace) = match validate_connector_and_resolve_account(
        &summary,
        &req.connector_id,
        &req.owner_address,
    ) {
        Ok(resolved) => resolved,
        Err((status, message)) => return (status, message).into_response(),
    };

    let input =
        delegation_sign_connector_approval_input(account_id, chain_namespace, &req, &context);
    let data = match create_connector_personal_sign_approval(&state, &authority, input).await {
        Ok(data) => data,
        Err((status, message)) => return (status, message).into_response(),
    };
    let Some(request_id) = data
        .get("approval_request")
        .and_then(|request| request.get("request_id"))
        .and_then(|value| value.as_str())
    else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "wallet-provider did not return a delegation-sign approval request id",
        )
            .into_response();
    };

    (
        StatusCode::OK,
        [("cache-control", "no-store")],
        Json(json!({
            "schema": "elastos.viewer.prepare-grant-sign/v1",
            "request_id": request_id,
            "connector_id": req.connector_id,
        })),
    )
        .into_response()
}

/// Pure request-shaping for the mint call — unit-testable without a `GatewayState` or a live
/// wallet-provider. Binds the approval's metadata to `(owner_address, kid)` (plus `uri` for audit
/// context) so a completed signature can never be replayed against a DIFFERENT asset's delegation.
fn delegation_sign_connector_approval_input(
    account_id: String,
    chain_namespace: String,
    req: &PrepareGrantSignRequest,
    context: &HomeLaunchTokenContext,
) -> ConnectorPersonalSignApprovalInput {
    ConnectorPersonalSignApprovalInput {
        account_id,
        chain_namespace,
        resource_action: DDRM_DELEGATION_SIGN_RESOURCE_ACTION.to_string(),
        reason: format!(
            "Confirm the dDRM delegation signature for content {}",
            req.kid
        ),
        payload: json!({
            "schema": "elastos.viewer.delegation-sign-request/v1",
            "method": "personal_sign",
            // The exact UTF-8 string the connector must EIP-191 personal_sign — byte-identical to
            // what `viewer_open::prepare_owned_grant` returned as `delegation_canonical`.
            "message": req.delegation_canonical,
            "owner_address": req.owner_address,
            "kid": req.kid,
            "uri": req.uri,
            "connector_id": req.connector_id,
            "principal_id": context.principal_id,
            "session_id": context.session_id,
            "requires_wallet_approval": true,
        }),
    }
}

/// GET /api/viewers/prepare-grant/sign/:request_id — report `{ status, delegation_sig_hex? }` by
/// reading the SAME approval store the connector completes into.
pub async fn delegation_sign_status_route(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    let required_launch = match require_owned_open_token_launch(&state.data_dir, &headers) {
        Ok(required) => required,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let authority = match runtime_wallet_authority(&required_launch) {
        Ok(authority) => authority,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    match delegation_sign_status(&state, &authority, &request_id).await {
        Ok(payload) => (
            StatusCode::OK,
            [("cache-control", "no-store")],
            Json(payload),
        )
            .into_response(),
        Err((status, message)) => (status, message).into_response(),
    }
}

async fn delegation_sign_status(
    state: &GatewayState,
    authority: &RuntimeWalletAuthority,
    request_id: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let response = runtime_wallet_data(
        state,
        authority,
        elastos_wallet_contract::WalletProviderOperationV2::ListApprovals {
            include_resolved: true,
        },
    )
    .await
    .map_err(|err| (StatusCode::SERVICE_UNAVAILABLE, err.to_string()))?;
    let approvals = response
        .get("approval_requests")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "wallet-provider returned an invalid approval list".to_string(),
            )
        })?;
    delegation_sign_status_from_approvals(
        approvals,
        request_id,
        authority.verified_context().actor(),
    )
}

/// Pure status-shaping core — unit-testable against a fabricated `approval_requests` array, no live
/// provider needed. Only matches an approval that is BOTH this endpoint's own intent
/// (`ddrm_delegation_sign`, never a Browser-bridge `browser_personal_sign`/`browser_typed_data_sign`
/// approval in the same per-principal store) AND was requested by the SAME actor the caller
/// authenticated as (`home`/`home-gui`/`home-cli`) — mirrors
/// `gateway_browser_wallet::browser_wallet_approval_status`'s `BROWSER_CAPSULE_ID` filter, just
/// scoped to whichever owned-open actor is calling instead of a single fixed one.
fn delegation_sign_status_from_approvals(
    approvals: &[serde_json::Value],
    request_id: &str,
    actor: &str,
) -> Result<serde_json::Value, (StatusCode, String)> {
    let request = approvals
        .iter()
        .find(|request| {
            request.get("request_id").and_then(|value| value.as_str()) == Some(request_id)
                && request.get("intent").and_then(|value| value.as_str())
                    == Some(DDRM_DELEGATION_SIGN_RESOURCE_ACTION)
                && request
                    .get("requested_by_actor")
                    .and_then(|value| value.as_str())
                    == Some(actor)
        })
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                "delegation-sign approval request not found".to_string(),
            )
        })?;
    let status = request
        .get("status")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown");
    let mut payload = json!({
        "schema": "elastos.viewer.prepare-grant-sign-status/v1",
        "request_id": request_id,
        "status": status,
    });
    if status == "completed" {
        let signature = request
            .get("signed_result")
            .and_then(|result| result.get("signature"))
            .and_then(|value| value.as_str())
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "completed delegation-sign approval is missing its signature".to_string(),
                )
            })?;
        payload["delegation_sig_hex"] = serde_json::Value::String(signature.to_string());
    }
    Ok(payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_request() -> PrepareGrantSignRequest {
        PrepareGrantSignRequest {
            uri: "elastos://owned/asset-1".to_string(),
            delegation_canonical: "delegation-canonical-text".to_string(),
            owner_address: "0x1111111111111111111111111111111111111111".to_string(),
            kid: "0xkid".to_string(),
            connector_id: "metamask-1".to_string(),
        }
    }

    fn sample_context() -> HomeLaunchTokenContext {
        HomeLaunchTokenContext {
            principal_id: "principal-1".to_string(),
            session_id: "session-1".to_string(),
            proof_binding_id: None,
            grant_id: "grant-1".to_string(),
        }
    }

    // --- Task 3: mint-input shaping ------------------------------------------------------------

    #[test]
    fn delegation_sign_input_carries_exact_message_resource_action_and_method() {
        let req = sample_request();
        let context = sample_context();
        let input = delegation_sign_connector_approval_input(
            "wallet:eip155:20:0x1111111111111111111111111111111111111111".to_string(),
            "eip155:20".to_string(),
            &req,
            &context,
        );

        assert_eq!(input.resource_action, "ddrm_delegation_sign");
        assert_eq!(input.payload["method"], "personal_sign");
        assert_eq!(input.payload["message"], "delegation-canonical-text");
    }

    /// The approval is bound to `(owner_address, kid)` so it cannot be reused for a different asset.
    #[test]
    fn delegation_sign_input_binds_metadata_to_owner_address_and_kid() {
        let req = sample_request();
        let context = sample_context();
        let input = delegation_sign_connector_approval_input(
            "wallet:eip155:20:0x1111111111111111111111111111111111111111".to_string(),
            "eip155:20".to_string(),
            &req,
            &context,
        );

        assert_eq!(
            input.payload["owner_address"],
            "0x1111111111111111111111111111111111111111"
        );
        assert_eq!(input.payload["kid"], "0xkid");
        assert_eq!(
            input.account_id,
            "wallet:eip155:20:0x1111111111111111111111111111111111111111"
        );
        assert_eq!(input.chain_namespace, "eip155:20");
    }

    // --- Task 3: status-shaping core ------------------------------------------------------------

    fn pending_approval(request_id: &str, actor: &str) -> serde_json::Value {
        json!({
            "request_id": request_id,
            "intent": "ddrm_delegation_sign",
            "requested_by_actor": actor,
            "status": "pending",
        })
    }

    fn completed_approval(request_id: &str, actor: &str, signature: &str) -> serde_json::Value {
        json!({
            "request_id": request_id,
            "intent": "ddrm_delegation_sign",
            "requested_by_actor": actor,
            "status": "completed",
            "signed_result": { "signature": signature },
        })
    }

    fn rejected_approval(request_id: &str, actor: &str) -> serde_json::Value {
        json!({
            "request_id": request_id,
            "intent": "ddrm_delegation_sign",
            "requested_by_actor": actor,
            "status": "rejected",
        })
    }

    #[test]
    fn status_reports_pending_before_completion() {
        let approvals = vec![pending_approval("req-1", "home-gui")];
        let payload =
            delegation_sign_status_from_approvals(&approvals, "req-1", "home-gui").unwrap();

        assert_eq!(payload["status"], "pending");
        assert!(payload.get("delegation_sig_hex").is_none());
    }

    /// REGRESSION target: simulates the connector completing the approval (as
    /// `CompleteConnectorHandoff` would write it into the store) — the status endpoint MUST surface
    /// the signature once `status` flips to `completed`.
    #[test]
    fn status_returns_signature_after_simulated_connector_completion() {
        let approvals = vec![completed_approval("req-1", "home-gui", "0xsignedbytes")];
        let payload =
            delegation_sign_status_from_approvals(&approvals, "req-1", "home-gui").unwrap();

        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["delegation_sig_hex"], "0xsignedbytes");
    }

    #[test]
    fn status_reports_rejected_without_a_signature() {
        let approvals = vec![rejected_approval("req-1", "home-gui")];
        let payload =
            delegation_sign_status_from_approvals(&approvals, "req-1", "home-gui").unwrap();

        assert_eq!(payload["status"], "rejected");
        assert!(payload.get("delegation_sig_hex").is_none());
    }

    #[test]
    fn status_is_not_found_for_an_unknown_request_id() {
        let approvals = vec![pending_approval("req-1", "home-gui")];
        let (status, _) =
            delegation_sign_status_from_approvals(&approvals, "req-does-not-exist", "home-gui")
                .unwrap_err();

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// A browser-bridge `browser_personal_sign` approval sharing the same store must never leak
    /// through this endpoint even if its `request_id` collided (it cannot in practice — request ids
    /// embed a nanosecond timestamp — but the intent filter is the real guarantee here).
    #[test]
    fn status_ignores_an_approval_with_a_different_intent() {
        let approvals = vec![json!({
            "request_id": "req-1",
            "intent": "browser_personal_sign",
            "requested_by_actor": "home-gui",
            "status": "completed",
            "signed_result": { "signature": "0xshouldnotleak" },
        })];
        let (status, _) =
            delegation_sign_status_from_approvals(&approvals, "req-1", "home-gui").unwrap_err();

        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    /// An approval requested by a different actor than the one currently authenticated must not be
    /// visible either (defense in depth — the wallet-provider store is already per-principal).
    #[test]
    fn status_ignores_an_approval_requested_by_a_different_actor() {
        let approvals = vec![pending_approval("req-1", "home-cli")];
        let (status, _) =
            delegation_sign_status_from_approvals(&approvals, "req-1", "home-gui").unwrap_err();

        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
