use super::*;

pub(in crate::api::gateway) async fn wallet_prices(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[WALLET_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    match wallet_prices_response(&state.data_dir, Some(&context)).await {
        Ok(response) => {
            if response.unavailable
                && response
                    .note
                    .as_deref()
                    .is_some_and(wallet_price_note_should_request_approval)
            {
                let _ = upsert_wallet_price_http_request(&state.data_dir, &context, now_ts());
            }
            Json(response).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_receive_qr(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<WalletQrRequest>,
) -> Response {
    if let Err(err) =
        require_home_launch_token_context(&state.data_dir, &headers, WALLET_CAPSULE_ID)
    {
        return system_error_response(err);
    }
    match wallet_qr_svg(&input.address) {
        Ok(svg) => Json(WalletQrResponse { svg }).into_response(),
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn wallet_prices_response(
    data_dir: &FsPath,
    context: Option<&HomeLaunchTokenContext>,
) -> anyhow::Result<WalletPricesResponse> {
    let now = now_ts();
    let cache = WALLET_PRICE_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    {
        let guard = cache.lock().await;
        if let Some(current) = guard.as_ref() {
            if now.saturating_sub(current.as_of) <= WALLET_PRICE_CACHE_TTL_SECS {
                return Ok(WalletPricesResponse {
                    as_of: current.as_of,
                    stale: false,
                    unavailable: false,
                    prices: current.prices.clone(),
                    note: None,
                });
            }
        }
    }

    let fetch_request_id = format!("{WALLET_PRICE_HTTP_REQUEST_ID}:fetch:{now}");
    if let Some(context) = context {
        append_wallet_price_fetch_audit(
            data_dir,
            context,
            &fetch_request_id,
            "requested",
            "Wallet requested approved external HTTP market-price fetch",
        )?;
    }
    match fetch_wallet_prices(data_dir, now).await {
        Ok(prices) => {
            let response = WalletPricesResponse {
                as_of: now,
                stale: false,
                unavailable: false,
                prices,
                note: None,
            };
            *cache.lock().await = Some(WalletPriceCache {
                as_of: response.as_of,
                prices: response.prices.clone(),
            });
            if let Some(context) = context {
                append_wallet_price_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "completed",
                    "Wallet completed approved external HTTP market-price fetch",
                )?;
            }
            Ok(response)
        }
        Err(err) => {
            if let Some(context) = context {
                let _ = append_wallet_price_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "failed",
                    "Wallet external HTTP market-price fetch failed or was blocked",
                );
            }
            let guard = cache.lock().await;
            if let Some(current) = guard.as_ref() {
                Ok(WalletPricesResponse {
                    as_of: current.as_of,
                    stale: true,
                    unavailable: false,
                    prices: current.prices.clone(),
                    note: Some(err.to_string()),
                })
            } else {
                Ok(wallet_prices_unavailable_response(now, err.to_string()))
            }
        }
    }
}

pub(in crate::api::gateway) fn wallet_prices_unavailable_response(
    now: u64,
    note: String,
) -> WalletPricesResponse {
    WalletPricesResponse {
        as_of: now,
        stale: true,
        unavailable: true,
        prices: BTreeMap::new(),
        note: Some(note),
    }
}

pub(in crate::api::gateway) async fn fetch_wallet_prices(
    data_dir: &FsPath,
    now: u64,
) -> anyhow::Result<BTreeMap<String, WalletPriceQuote>> {
    validate_wallet_price_source(data_dir)?;
    let ids = WALLET_PRICE_IDS
        .iter()
        .map(|(_, id)| *id)
        .collect::<Vec<_>>()
        .join(",");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent("ElastOS-Wallet/0.2")
        .build()?;
    let mut request = client.get(WALLET_PRICE_API_URL).query(&[
        ("ids", ids.as_str()),
        ("vs_currencies", "usd"),
        ("include_24hr_change", "true"),
    ]);
    if let Ok(api_key) = std::env::var("COINGECKO_DEMO_API_KEY") {
        let api_key = api_key.trim().to_string();
        if !api_key.is_empty() {
            request = request.header("x-cg-demo-api-key", api_key);
        }
    }
    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!("price source returned {}: {}", status, body.trim());
    }
    let payload = response.json::<serde_json::Value>().await?;
    wallet_prices_from_coingecko_payload(&payload, now)
}

pub(in crate::api::gateway) fn validate_wallet_price_source(
    data_dir: &FsPath,
) -> anyhow::Result<()> {
    wallet_price_source_decision(
        |name| std::env::var(name).ok(),
        || load_wallet_price_policy(data_dir).ok(),
    )
}

pub(in crate::api::gateway) fn wallet_price_source_decision<F, P>(
    get_env: F,
    get_policy: P,
) -> anyhow::Result<()>
where
    F: Fn(&str) -> Option<String>,
    P: Fn() -> Option<WalletPricePolicy>,
{
    let policy = get_policy();
    let source = get_env(WALLET_PRICE_SOURCE_ENV)
        .or_else(|| policy.as_ref().map(|policy| policy.source.clone()))
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if source.is_empty() {
        anyhow::bail!(
            "wallet price source is not configured; use a typed oracle/provider source or explicitly approve an external HTTP source"
        );
    }
    if source != "coingecko" {
        anyhow::bail!("unsupported wallet price source: {source}");
    }
    let approved = get_env(WALLET_PRICE_HTTP_APPROVED_ENV)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let policy_approved = policy
        .as_ref()
        .is_some_and(|policy| policy.external_http_approved && policy.source == "coingecko");
    if !matches!(approved.as_str(), "1" | "true" | "yes" | "approved") && !policy_approved {
        anyhow::bail!("external wallet price HTTP source is not approved");
    }
    Ok(())
}

pub(in crate::api::gateway) fn wallet_price_note_should_request_approval(note: &str) -> bool {
    note.contains("wallet price source is not configured")
        || note.contains("external wallet price HTTP source is not approved")
}

pub(in crate::api::gateway) fn wallet_price_policy_path(
    data_dir: &FsPath,
) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, WALLET_PRICE_POLICY_ROOT)
        .ok_or_else(|| anyhow::anyhow!("invalid wallet price policy root"))
        .map(|root| root.join(WALLET_PRICE_POLICY_FILE))
}

pub(in crate::api::gateway) fn load_wallet_price_policy(
    data_dir: &FsPath,
) -> anyhow::Result<WalletPricePolicy> {
    let path = wallet_price_policy_path(data_dir)?;
    let bytes = std::fs::read(&path)?;
    let policy = serde_json::from_slice::<WalletPricePolicy>(&bytes)?;
    if policy.schema != WALLET_PRICE_POLICY_SCHEMA {
        anyhow::bail!("unsupported wallet price policy schema");
    }
    Ok(policy)
}

pub(in crate::api::gateway) fn store_wallet_price_http_policy(
    data_dir: &FsPath,
    principal_id: &str,
    approved_at: u64,
) -> anyhow::Result<()> {
    let policy = WalletPricePolicy {
        schema: WALLET_PRICE_POLICY_SCHEMA.to_string(),
        source: "coingecko".to_string(),
        external_http_approved: true,
        approved_by_principal_id: principal_id.to_string(),
        approved_at,
    };
    let path = wallet_price_policy_path(data_dir)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_vec_pretty(&policy)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
}

pub(in crate::api::gateway) fn upsert_wallet_price_http_request(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    created_at: u64,
) -> anyhow::Result<()> {
    crate::notifications::upsert_external_http_request(
        data_dir,
        WALLET_PRICE_HTTP_REQUEST_ID,
        WALLET_CAPSULE_ID,
        "Wallet requests market prices",
        "Wallet wants approved HTTP access to CoinGecko for BTC, ELA, ETH, USDC, and USDT prices.",
        WALLET_PRICE_HTTP_APPROVE_ACTION_ID,
        created_at,
    )?;
    append_wallet_price_policy_audit(
        data_dir,
        &context.principal_id,
        &context.session_id,
        "requested",
        "Wallet price HTTP access requested",
    )
}

pub(in crate::api::gateway) fn ensure_admin_context(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
) -> anyhow::Result<()> {
    let Some(proof_binding_id) = context.proof_binding_id.as_deref() else {
        anyhow::bail!("admin passkey required");
    };
    let principal = crate::auth::load_principal_for_proof_binding(data_dir, proof_binding_id)?;
    crate::auth::ensure_proof_binding_not_revoked(&principal)?;
    if !crate::auth::is_admin(&principal) {
        anyhow::bail!("admin passkey required");
    }
    Ok(())
}

pub(in crate::api::gateway) fn append_wallet_price_policy_audit(
    data_dir: &FsPath,
    principal_id: &str,
    session_id: &str,
    result: &str,
    reason: &str,
) -> anyhow::Result<()> {
    append_wallet_approval_audit(
        data_dir,
        WalletApprovalAuditInput {
            capsule_id: WALLET_CAPSULE_ID,
            event_type: "wallet.price_source.policy",
            principal_id,
            session_id,
            request_id: WALLET_PRICE_HTTP_REQUEST_ID,
            result,
            reason,
        },
    )
}

pub(in crate::api::gateway) fn append_wallet_price_fetch_audit(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    request_id: &str,
    result: &str,
    reason: &str,
) -> anyhow::Result<()> {
    let event_type = match result {
        "requested" => "wallet.price_source.fetch.requested",
        "completed" => "wallet.price_source.fetch.completed",
        "failed" => "wallet.price_source.fetch.failed",
        _ => "wallet.price_source.fetch",
    };
    append_wallet_approval_audit(
        data_dir,
        WalletApprovalAuditInput {
            capsule_id: WALLET_CAPSULE_ID,
            event_type,
            principal_id: &context.principal_id,
            session_id: &context.session_id,
            request_id,
            result,
            reason,
        },
    )
}

pub(in crate::api::gateway) fn wallet_prices_from_coingecko_payload(
    payload: &serde_json::Value,
    _now: u64,
) -> anyhow::Result<BTreeMap<String, WalletPriceQuote>> {
    let mut prices = BTreeMap::new();
    for (symbol, id) in WALLET_PRICE_IDS {
        let Some(item) = payload.get(*id) else {
            continue;
        };
        let Some(usd) = item.get("usd").and_then(|value| value.as_f64()) else {
            continue;
        };
        let change_24h = item
            .get("usd_24h_change")
            .and_then(|value| value.as_f64())
            .unwrap_or(0.0);
        if usd.is_finite() && change_24h.is_finite() {
            prices.insert((*symbol).to_string(), WalletPriceQuote { usd, change_24h });
        }
    }
    if prices.is_empty() {
        anyhow::bail!("price source returned no supported assets");
    }
    Ok(prices)
}

pub(in crate::api::gateway) fn wallet_qr_svg(address: &str) -> anyhow::Result<String> {
    let address = address.trim();
    if address.is_empty()
        || address.len() > 256
        || address
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        anyhow::bail!("invalid receive address");
    }
    let qr = qrcodegen::QrCode::encode_text(address, qrcodegen::QrCodeEcc::Medium)
        .map_err(|err| anyhow::anyhow!("could not encode receive QR: {err:?}"))?;
    let size = qr.size();
    let border = 3;
    let view_size = size + border * 2;
    let mut path = String::new();
    for y in 0..size {
        for x in 0..size {
            if qr.get_module(x, y) {
                path.push_str(&format!("M{} {}h1v1h-1z", x + border, y + border));
            }
        }
    }
    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {view_size} {view_size}" shape-rendering="crispEdges" role="img" aria-label="Receive address QR"><rect width="100%" height="100%" fill="white"/><path d="{path}" fill="black"/></svg>"#
    ))
}
