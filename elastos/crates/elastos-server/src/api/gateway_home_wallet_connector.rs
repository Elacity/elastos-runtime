use super::*;
use crate::api::auth_gateway;
use serde_json::json;

const HOME_WALLET_CONNECTOR_REQUEST_SCHEMA: &str =
    "elastos.home.wallet-connector.effect.request/v1";
const HOME_WALLET_CONNECTOR_RESULT_SCHEMA: &str = "elastos.home.wallet-connector.effect.result/v1";
const INJECTED_WALLET_CONNECTOR_IDS: &[&str] =
    &[WALLET_METAMASK_CAPSULE_ID, WALLET_UNISAT_CAPSULE_ID];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorAuthorityRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorEvmChallengeRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
    address: String,
    chain_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorEvmVerifyRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
    message: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorBitcoinChallengeRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
    address: String,
    network: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorBitcoinVerifyRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
    message: String,
    signature: String,
    signature_type: String,
    #[serde(default)]
    public_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeWalletConnectorApprovalCompleteRequest {
    schema: String,
    connector_id: String,
    connector_token: String,
    payload_hash: String,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    signature_type: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
    signer: String,
    #[serde(default)]
    transaction_hash: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeWalletConnectorHandoff {
    schema: String,
    request_id: String,
    intent: String,
    payload_hash: String,
    signer: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    transaction: Option<HomeWalletConnectorTransaction>,
    status: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HomeWalletConnectorTransaction {
    from: String,
    to: String,
    value: String,
    data: String,
    gas: String,
    #[serde(rename = "gasPrice")]
    gas_price: String,
    nonce: String,
    #[serde(rename = "chainId")]
    chain_id: String,
}

struct HomeWalletConnectorAuthority {
    context: HomeLaunchTokenContext,
    wallet: RuntimeWalletAuthority,
}

pub(super) async fn home_wallet_connector_evm_link_challenge(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeWalletConnectorEvmChallengeRequest>,
) -> Response {
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        WALLET_METAMASK_CAPSULE_ID,
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    let link = match auth_gateway::verified_wallet_link_context(
        &state,
        &input.connector_id,
        authority.wallet,
    ) {
        Ok(link) => link,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match auth_gateway::evm_challenge_for_wallet_link(
        &state,
        &headers,
        auth_gateway::EvmChallengeRequest {
            address: input.address,
            chain_id: input.chain_id,
        },
        link,
    )
    .await
    {
        Ok(challenge) => Json(json!({
            "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
            "action": "evm_link_challenge",
            "connector_id": input.connector_id,
            "challenge": challenge,
        }))
        .into_response(),
        Err(err) => auth_gateway::auth_error_response(err),
    }
}

pub(super) async fn home_wallet_connector_evm_link_verify(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeWalletConnectorEvmVerifyRequest>,
) -> Response {
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        WALLET_METAMASK_CAPSULE_ID,
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    let link = match auth_gateway::verified_wallet_link_context(
        &state,
        &input.connector_id,
        authority.wallet,
    ) {
        Ok(link) => link,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match auth_gateway::evm_verify_for_wallet_link(
        &state,
        auth_gateway::EvmVerifyRequest {
            message: input.message,
            signature: input.signature,
        },
        link,
    )
    .await
    {
        Ok(_) => Json(json!({
            "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
            "action": "evm_link_complete",
            "connector_id": input.connector_id,
            "status": "linked",
        }))
        .into_response(),
        Err(err) => auth_gateway::auth_error_response(err),
    }
}

pub(super) async fn home_wallet_connector_bitcoin_link_challenge(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeWalletConnectorBitcoinChallengeRequest>,
) -> Response {
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        WALLET_UNISAT_CAPSULE_ID,
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    let link = match auth_gateway::verified_wallet_link_context(
        &state,
        &input.connector_id,
        authority.wallet,
    ) {
        Ok(link) => link,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match auth_gateway::btc_challenge_for_wallet_link(
        &state,
        &headers,
        auth_gateway::BtcChallengeRequest {
            address: input.address,
            network: input.network,
        },
        link,
    )
    .await
    {
        Ok(challenge) => Json(json!({
            "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
            "action": "bitcoin_link_challenge",
            "connector_id": input.connector_id,
            "challenge": challenge,
        }))
        .into_response(),
        Err(err) => auth_gateway::auth_error_response(err),
    }
}

pub(super) async fn home_wallet_connector_bitcoin_link_verify(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeWalletConnectorBitcoinVerifyRequest>,
) -> Response {
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        WALLET_UNISAT_CAPSULE_ID,
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    let link = match auth_gateway::verified_wallet_link_context(
        &state,
        &input.connector_id,
        authority.wallet,
    ) {
        Ok(link) => link,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match auth_gateway::btc_verify_for_wallet_link(
        &state,
        auth_gateway::BtcVerifyRequest {
            message: input.message,
            signature: input.signature,
            signature_type: Some(input.signature_type),
            public_key: input.public_key,
        },
        link,
    )
    .await
    {
        Ok(_) => Json(json!({
            "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
            "action": "bitcoin_link_complete",
            "connector_id": input.connector_id,
            "status": "linked",
        }))
        .into_response(),
        Err(err) => auth_gateway::auth_error_response(err),
    }
}

pub(super) async fn home_wallet_connector_approval_handoff(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<HomeWalletConnectorAuthorityRequest>,
) -> Response {
    if let Err(err) = validate_home_wallet_connector_request_id(&request_id) {
        return home_wallet_connector_request_error_response(err);
    }
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        input.connector_id.as_str(),
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match approve_external_wallet_request(
        &state,
        &state.data_dir,
        &authority.context,
        &authority.wallet,
        &request_id,
        "Approved through trusted Home injected-wallet host",
        &input.connector_id,
    )
    .await
    {
        Ok(outcome) => {
            let Some(handoff) = outcome.handoff else {
                return system_error_response(anyhow::anyhow!(
                    "Wallet connector handoff is unavailable"
                ));
            };
            let handoff = match serde_json::from_value::<HomeWalletConnectorHandoff>(handoff)
                .and_then(|handoff| {
                    validate_home_wallet_connector_handoff(
                        &handoff,
                        &request_id,
                        &input.connector_id,
                    )
                    .map(|_| handoff)
                    .map_err(serde::de::Error::custom)
                }) {
                Ok(handoff) => handoff,
                Err(err) => {
                    return system_error_response(anyhow::anyhow!(
                        "Wallet connector returned an invalid typed handoff: {err}"
                    ));
                }
            };
            Json(json!({
                "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
                "action": "approval_handoff",
                "connector_id": input.connector_id,
                "request_id": request_id,
                "handoff": handoff,
                "evm_chains": wallet_connector_evm_chains(),
            }))
            .into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(super) async fn home_wallet_connector_approval_complete(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(input): Json<HomeWalletConnectorApprovalCompleteRequest>,
) -> Response {
    if let Err(err) = validate_home_wallet_connector_request_id(&request_id) {
        return home_wallet_connector_request_error_response(err);
    }
    let authority = match require_home_wallet_connector_authority(
        &state,
        &headers,
        &input.schema,
        &input.connector_id,
        &input.connector_token,
        input.connector_id.as_str(),
    ) {
        Ok(authority) => authority,
        Err(err) => return home_wallet_connector_authority_error_response(err),
    };
    match complete_external_wallet_approval(
        &state,
        &authority.context,
        &authority.wallet,
        &request_id,
        WalletApprovalCompleteRequest {
            payload_hash: input.payload_hash,
            signature: input.signature,
            signature_type: input.signature_type,
            public_key: input.public_key,
            signer: input.signer,
            transaction_hash: input.transaction_hash,
        },
        &input.connector_id,
        "External wallet effect completed through trusted Home injected-wallet host",
    )
    .await
    {
        Ok(_) => Json(json!({
            "schema": HOME_WALLET_CONNECTOR_RESULT_SCHEMA,
            "action": "approval_complete",
            "connector_id": input.connector_id,
            "request_id": request_id,
            "status": "completed",
        }))
        .into_response(),
        Err(err) => system_error_response(err),
    }
}

fn require_home_wallet_connector_authority(
    state: &GatewayState,
    headers: &HeaderMap,
    schema: &str,
    connector_id: &str,
    connector_token: &str,
    expected_connector_id: &str,
) -> anyhow::Result<HomeWalletConnectorAuthority> {
    if schema != HOME_WALLET_CONNECTOR_REQUEST_SCHEMA {
        anyhow::bail!("unsupported Home wallet-connector request schema");
    }
    if connector_id != expected_connector_id
        || !INJECTED_WALLET_CONNECTOR_IDS.contains(&connector_id)
    {
        anyhow::bail!("Home wallet-connector actor mismatch");
    }
    ensure_wallet_connector_configured(&state.data_dir, connector_id)?;

    let home = require_home_runtime_wallet_authority(&state.data_dir, headers)?;
    let connector_launch =
        require_carried_home_launch_token(&state.data_dir, connector_token, &[connector_id])?;
    if connector_launch.launch_context.selected_resource != connector_id
        || connector_launch.launch_context.executable_actor != connector_id
        || connector_launch.launch_context.authority_actor != HOME_CAPSULE_ID
    {
        anyhow::bail!("Home wallet-connector launch context mismatch");
    }
    let wallet = runtime_wallet_authority(&connector_launch)?;
    let home_context = home.verified_context();
    let connector_context = wallet.verified_context();
    if home_context.principal_id() != connector_context.principal_id()
        || home_context.session_id() != connector_context.session_id()
        || home_context.proof_binding_id() != connector_context.proof_binding_id()
        || home_context.grant_id() != connector_context.grant_id()
        || connector_context.proof_binding_id().is_none()
        || connector_context.actor() != connector_id
    {
        anyhow::bail!("Home and wallet-connector launch authority mismatch");
    }

    Ok(HomeWalletConnectorAuthority {
        context: wallet.home_launch_context(),
        wallet,
    })
}

fn validate_home_wallet_connector_request_id(request_id: &str) -> anyhow::Result<()> {
    if request_id.is_empty() || request_id.len() > 256 || request_id.chars().any(char::is_control) {
        anyhow::bail!("invalid wallet approval request ID");
    }
    Ok(())
}

fn home_wallet_connector_authority_error_response(err: anyhow::Error) -> Response {
    (StatusCode::FORBIDDEN, err.to_string()).into_response()
}

fn home_wallet_connector_request_error_response(err: anyhow::Error) -> Response {
    (StatusCode::BAD_REQUEST, err.to_string()).into_response()
}

fn validate_home_wallet_connector_handoff(
    handoff: &HomeWalletConnectorHandoff,
    request_id: &str,
    connector_id: &str,
) -> anyhow::Result<()> {
    if handoff.schema != "elastos.wallet.webconnect_handoff/v1"
        || handoff.request_id != request_id
        || handoff.intent.is_empty()
        || handoff.intent.len() > 128
        || handoff.intent.chars().any(char::is_control)
        || handoff.payload_hash.len() != 66
        || !handoff.payload_hash.starts_with("0x")
        || !handoff.payload_hash[2..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || handoff.signer.is_empty()
        || handoff.signer.len() > 256
        || handoff.signer.chars().any(char::is_control)
    {
        anyhow::bail!("Wallet connector handoff binding is invalid");
    }

    match connector_id {
        WALLET_METAMASK_CAPSULE_ID if handoff.intent == "bitcoin_bip322_proof" => {
            anyhow::bail!("MetaMask handoff cannot carry a Bitcoin approval")
        }
        WALLET_UNISAT_CAPSULE_ID if handoff.intent != "bitcoin_bip322_proof" => {
            anyhow::bail!("UniSat handoff must carry a Bitcoin approval")
        }
        WALLET_METAMASK_CAPSULE_ID if handoff.intent == "transaction_intent" => {
            if handoff.status != "awaiting_wallet_transaction"
                || handoff.transaction.is_none()
                || handoff.message.is_some()
                || handoff.signature_type.is_some()
            {
                anyhow::bail!("Wallet connector transaction handoff is invalid");
            }
        }
        WALLET_METAMASK_CAPSULE_ID => {
            if handoff.status != "awaiting_wallet_signature"
                || handoff.transaction.is_some()
                || handoff.message.is_none()
                || handoff.signature_type.as_deref() != Some("personal_sign")
            {
                anyhow::bail!("Wallet connector signature handoff is invalid");
            }
        }
        WALLET_UNISAT_CAPSULE_ID => {
            if handoff.status != "awaiting_wallet_signature"
                || handoff.transaction.is_some()
                || handoff.message.is_none()
                || !matches!(
                    handoff.signature_type.as_deref(),
                    Some("bip322_simple" | "ecdsa")
                )
            {
                anyhow::bail!("Wallet connector Bitcoin handoff is invalid");
            }
        }
        _ => anyhow::bail!("unknown injected-wallet connector"),
    }
    Ok(())
}
