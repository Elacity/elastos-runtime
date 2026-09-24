//! The Marketplace's view of on-chain state: the channels that publish
//! protected items, and what this Home's account already holds on them.
//!
//! The channel list is read from the index described in
//! `gateway_onchain_directory`; what the account holds is read from the chain
//! itself, because it decides what a card offers. Neither is authority: the
//! index says which channels exist, the chain decides every access and every
//! payment when it happens, and a Home that cannot reach the index loses a
//! view and no capability at all.
//!
//! The account is resolved inside the Runtime from the launch context's wallet
//! authority and is never returned to the page. A capsule that never had to
//! learn this Home's address should not learn it to decide which button to
//! draw.

use super::*;

static MARKET_CHANNELS_CACHE: OnceLock<tokio::sync::Mutex<Option<MarketChannelsCache>>> =
    OnceLock::new();

struct MarketChannelsCache {
    as_of: u64,
    channels: Vec<MarketChannelEntry>,
}

/// The Marketplace's directory. Its own request, policy and audit stream: the
/// Creator's approval does not answer for it, nor it for the Creator's.
pub(in crate::api::gateway) const MARKET_DIRECTORY: OnchainDirectory = OnchainDirectory {
    capsule_id: MARKETPLACE_CAPSULE_ID,
    subject: "market directory",
    request_id: MARKETPLACE_DIRECTORY_HTTP_REQUEST_ID,
    approve_action_id: MARKETPLACE_DIRECTORY_HTTP_APPROVE_ACTION_ID,
    deny_action_prefix: MARKETPLACE_DIRECTORY_HTTP_DENY_ACTION_PREFIX,
    deny_action_suffix: "onchain-graphql",
    source_env: MARKETPLACE_DIRECTORY_SOURCE_ENV,
    approved_env: MARKETPLACE_DIRECTORY_HTTP_APPROVED_ENV,
    policy_schema: MARKETPLACE_DIRECTORY_POLICY_SCHEMA,
    policy_root: MARKETPLACE_DIRECTORY_POLICY_ROOT,
    policy_file: MARKETPLACE_DIRECTORY_POLICY_FILE,
    audit_prefix: "marketplace.market_directory",
    request_title: "Marketplace requests the channel list",
    request_body: "Marketplace wants approved HTTP access to an on-chain index, to show the channels publishing protected items. Buying, opening and listing all work without it.",
    user_agent: "ElastOS-Marketplace/0.1",
};

/// `GET /api/apps/marketplace/channels` -- the Shops surface.
pub(in crate::api::gateway) async fn marketplace_channels(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    match market_channels_response(&state.data_dir, Some(&context)).await {
        Ok(response) => {
            // Asking is what raises the approval, so a Home whose owner never
            // opened Shops is never asked about it.
            if response.needs_approval {
                let _ = MARKET_DIRECTORY.upsert_request(&state.data_dir, &context, now_ts());
            }
            Json(response).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn market_channels_response(
    data_dir: &FsPath,
    context: Option<&HomeLaunchTokenContext>,
) -> anyhow::Result<MarketChannelsResponse> {
    let now = now_ts();
    let cache = MARKET_CHANNELS_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    {
        let guard = cache.lock().await;
        if let Some(current) = guard.as_ref() {
            if now.saturating_sub(current.as_of) <= MARKETPLACE_DIRECTORY_CACHE_TTL_SECS {
                return Ok(MarketChannelsResponse {
                    as_of: current.as_of,
                    stale: false,
                    unavailable: false,
                    needs_approval: false,
                    channels: current.channels.clone(),
                    note: None,
                });
            }
        }
    }

    let fetch_request_id = format!("{MARKETPLACE_DIRECTORY_HTTP_REQUEST_ID}:channels:{now}");
    if let Some(context) = context {
        let _ = MARKET_DIRECTORY.append_fetch_audit(
            data_dir,
            context,
            &fetch_request_id,
            "requested",
            "Marketplace requested approved external HTTP channel-directory fetch",
        );
    }
    match fetch_market_channels(data_dir).await {
        Ok(channels) => {
            if let Some(context) = context {
                let _ = MARKET_DIRECTORY.append_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "completed",
                    "Marketplace completed approved external HTTP channel-directory fetch",
                );
            }
            let mut guard = cache.lock().await;
            *guard = Some(MarketChannelsCache {
                as_of: now,
                channels: channels.clone(),
            });
            Ok(MarketChannelsResponse {
                as_of: now,
                stale: false,
                unavailable: false,
                needs_approval: false,
                channels,
                note: None,
            })
        }
        Err(err) => {
            let note = err.to_string();
            if let Some(context) = context {
                let _ = MARKET_DIRECTORY.append_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "failed",
                    &note,
                );
            }
            // A directory that cannot be read is a surface with nothing on it
            // and a reason, never an error that stops the app.
            let needs_approval = MARKET_DIRECTORY.note_should_request_approval(&note);
            Ok(MarketChannelsResponse {
                as_of: now,
                stale: false,
                unavailable: true,
                needs_approval,
                channels: Vec::new(),
                note: Some(note),
            })
        }
    }
}

/// `POST /api/apps/marketplace/channel-access` -- which Shops cards should
/// offer a subscription.
///
/// A second call rather than part of the channel list, because the list comes
/// from one HTTP request and this is two chain reads per channel across
/// several sources. Folding them together would make a directory of fifty
/// channels wait on two hundred RPC calls before a single card appeared.
///
/// The page sends the channels it is showing and learns their state. It does
/// not send, and never learns, the account those states are about.
pub(in crate::api::gateway) async fn marketplace_channel_access(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(request): Json<MarketChannelAccessRequest>,
) -> Response {
    if let Err(err) = require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        return system_error_response(err);
    }
    let authority = match require_app_runtime_wallet_authority(
        &state.data_dir,
        &headers,
        &[MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    if request.channels.len() > MARKET_CHANNEL_ACCESS_MAX {
        return Json(serde_json::json!({
            "channels": [],
            "unavailable": true,
            "note": "too many channels in one request",
        }))
        .into_response();
    }
    let channels: Vec<String> = request
        .channels
        .iter()
        .map(|channel| channel.trim().to_ascii_lowercase())
        .filter(|channel| is_evm_address_lowercase(channel))
        .collect();
    match runtime_custody_channel_access(&state, &authority, &channels).await {
        Ok(states) => Json(serde_json::json!({
            "channels": states
                .into_iter()
                .map(|(channel, state)| serde_json::json!({ "channel": channel, "state": state }))
                .collect::<Vec<_>>(),
            "unavailable": false,
        }))
        .into_response(),
        Err(error) => {
            // Not an error the surface stops for. Every card simply does not
            // know yet, which is a state it already renders.
            tracing::debug!(%error, "marketplace channel access unavailable");
            Json(serde_json::json!({
                "channels": [],
                "unavailable": true,
            }))
            .into_response()
        }
    }
}

/// One page of Shops. Larger than this is a caller asking for more than the
/// surface shows.
const MARKET_CHANNEL_ACCESS_MAX: usize = 64;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::api::gateway) struct MarketChannelAccessRequest {
    #[serde(default)]
    pub channels: Vec<String>,
}

/// `GET /api/apps/marketplace/pay-tokens` -- what a price is denominated in.
///
/// A listing carries a price as a uint256 and the address it is priced in.
/// Neither says how many decimals that token has or what it is called, so a
/// shelf showing "100000" is showing a number nobody can read.
///
/// The allow-list is local configuration -- the same one a mint is priced
/// against -- so this answers whether or not any external service is
/// reachable, and a token missing from it is shown in base units rather than
/// guessed at.
pub(in crate::api::gateway) async fn marketplace_pay_tokens(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    if let Err(err) = require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[
            MARKETPLACE_CAPSULE_ID,
            LIBRARY_CAPSULE_ID,
            SYSTEM_CAPSULE_ID,
        ],
    ) {
        return system_error_response(err);
    }
    match runtime_custody_pay_token_table(&state).await {
        Ok(tokens) => Json(serde_json::json!({ "payTokens": tokens })).into_response(),
        Err(error) => {
            // A shelf without this shows base units, which is honest and
            // useless rather than wrong.
            tracing::debug!(%error, "marketplace pay tokens unavailable");
            Json(serde_json::json!({ "payTokens": [] })).into_response()
        }
    }
}

/// `GET /api/apps/marketplace/items` -- what anyone has minted.
///
/// Explore shows the market rather than this Home's own shelf, so the items
/// come from the index. What this Home already holds is merged over them by
/// the capsule, which knows its own listings: the index says what exists, and
/// Runtime says what this person may do about it.
pub(in crate::api::gateway) async fn marketplace_catalog_items(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[MARKETPLACE_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    // Prices arrive in token units and every other price on this Home is in
    // base units, so the table that says how to convert is read first. It is
    // local configuration; an index being reachable does not make it so.
    let pay_tokens: Vec<(String, u8)> = runtime_custody_pay_token_table(&state)
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|token| {
            Some((
                token.get("address")?.as_str()?.to_ascii_lowercase(),
                u8::try_from(token.get("decimals")?.as_u64()?).ok()?,
            ))
        })
        .collect();
    let now = now_ts();
    let fetch_request_id = format!("{MARKETPLACE_DIRECTORY_HTTP_REQUEST_ID}:catalog:{now}");
    let _ = MARKET_DIRECTORY.append_fetch_audit(
        &state.data_dir,
        &context,
        &fetch_request_id,
        "requested",
        "Marketplace requested approved external HTTP catalog fetch",
    );
    match fetch_market_catalog(&state.data_dir, &pay_tokens).await {
        Ok(mut items) => {
            // What this Home already holds, joined on the names the chain
            // uses. Done here because the channel an item lives on is not
            // published to a surface, so a surface could not make this join.
            let held = crate::protected_content_runtime::runtime_custody_listing_chain_index(
                &state.data_dir,
                &context.principal_id,
            )
            .unwrap_or_default();
            for item in &mut items {
                if let Some(local) = held
                    .iter()
                    .find(|entry| entry.ledger == item.ledger && entry.token_id == item.token_id)
                {
                    item.mint_id = local.mint_id.clone();
                    item.access_state = local.access_state.clone();
                }
            }
            let _ = MARKET_DIRECTORY.append_fetch_audit(
                &state.data_dir,
                &context,
                &fetch_request_id,
                "completed",
                "Marketplace completed approved external HTTP catalog fetch",
            );
            Json(serde_json::json!({
                "asOf": now,
                "unavailable": false,
                "needsApproval": false,
                "items": items,
            }))
            .into_response()
        }
        Err(error) => {
            let note = error.to_string();
            // Said in the log as well as audited. The audit chain stores an
            // event id and a hash, so a reason recorded only there cannot be
            // read back when someone asks why a shelf is empty.
            tracing::warn!(%note, "marketplace catalog fetch failed");
            let _ = MARKET_DIRECTORY.append_fetch_audit(
                &state.data_dir,
                &context,
                &fetch_request_id,
                "failed",
                &note,
            );
            let needs_approval = MARKET_DIRECTORY.note_should_request_approval(&note);
            if needs_approval {
                let _ = MARKET_DIRECTORY.upsert_request(&state.data_dir, &context, now);
            }
            // An index that cannot be read leaves Explore with this Home's own
            // items and a reason, never an error that stops the app.
            Json(serde_json::json!({
                "asOf": now,
                "unavailable": true,
                "needsApproval": needs_approval,
                "items": [],
                "note": note,
            }))
            .into_response()
        }
    }
}

pub(in crate::api::gateway) async fn fetch_market_catalog(
    data_dir: &FsPath,
    pay_tokens: &[(String, u8)],
) -> anyhow::Result<Vec<MarketCatalogItem>> {
    MARKET_DIRECTORY.validate_source(data_dir)?;
    let payload = MARKET_DIRECTORY
        .post_graphql(
            MARKET_CATALOG_QUERY,
            serde_json::json!({
                "query": market_catalog_query_input(),
                "filters": { "limit": MARKET_CATALOG_PAGE_LIMIT, "offset": 0 },
            }),
        )
        .await?;
    market_catalog_from_payload(&payload, pay_tokens)
}

/// The catalog: what anyone has minted, as this Home can show it.
///
/// The index answers in its own dialect, and every value below is converted
/// before it reaches a capsule, because the alternative is each surface
/// learning the index's shapes for itself:
///
/// ```text
/// price        0.1                 a float in TOKEN units   -> base units, as a uint256 string
/// createdAt    1790138251000       milliseconds             -> seconds
/// kid          "27df70c5…"         no 0x                    -> 0x-prefixed
/// tokenURI     ipfs://Qm…/metadata.json                     -> the directory CID
/// media.uri    ipfs://Qm…                                   -> the content CID
/// contentType  "video"             a category, not a MIME   -> carried as a category
/// ```
///
/// None of it is authority. A price shown here is the index's claim; the terms
/// of a sale are read from chain before anything is signed, exactly as they
/// are for a listing this Home already holds.
pub(in crate::api::gateway) const MARKET_CATALOG_QUERY: &str = "query FetchNFTItems($query: NFTItemQueryInput, $filters: FilterPaginationInput) { result: fetchNFTItems(query: $query, filters: $filters) { total data { __typename ... on ProtectedAsset { contractAddress hexTokenID name description tokenURI imageURL price paymentToken priceInUSD views createdAt unpublished metadata { kid media { uri contentType size } } operative { address opType contentId owner access { totalSupply listings { seller price quantity payToken } } } } } } }";

/// One page of the shelf. The index counts in the hundreds; a surface that
/// shows three of each kind does not need them all at once.
pub(in crate::api::gateway) const MARKET_CATALOG_PAGE_LIMIT: u32 = 60;

/// `type` and `filterby` are required by the index's own validator -- a query
/// without them is refused with a 422 rather than answered -- and `variant`
/// narrows it to protected assets, which are the only ones this Home can do
/// anything with.
pub(in crate::api::gateway) fn market_catalog_query_input() -> serde_json::Value {
    serde_json::json!({ "type": "single", "variant": "drm", "filterby": [] })
}

pub(in crate::api::gateway) fn market_catalog_from_payload(
    payload: &serde_json::Value,
    pay_tokens: &[(String, u8)],
) -> anyhow::Result<Vec<MarketCatalogItem>> {
    let entries = MARKET_DIRECTORY.rows(payload, "result")?;
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        // An item the index has withdrawn is not on anyone's shelf.
        if entry
            .get("unpublished")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            continue;
        }
        let Some(item) = market_catalog_item(entry, pay_tokens) else {
            continue;
        };
        items.push(item);
    }
    Ok(items)
}

/// One row, or nothing.
///
/// A row missing anything this Home would need to act on it is dropped rather
/// than shown as a card that cannot do what its buttons say. The ledger, the
/// token and the content id are that minimum: without them there is no asset
/// to name, no chain to ask about it, and no content to open.
fn market_catalog_item(
    entry: &serde_json::Value,
    pay_tokens: &[(String, u8)],
) -> Option<MarketCatalogItem> {
    let ledger = evm_address(entry.get("contractAddress"))?;
    let token_id = hex_quantity(entry.get("hexTokenID"))?;
    let metadata = entry.get("metadata");
    let content_access_id = content_access_id(metadata.and_then(|value| value.get("kid")))?;
    let operative = entry.get("operative");
    let listing = operative
        .and_then(|value| value.get("access"))
        .and_then(|value| value.get("listings"))
        .and_then(serde_json::Value::as_array)
        .and_then(|listings| listings.first());
    // The listing's own terms when the index has them, and the item's
    // headline price when it does not. Both are the index's claim and neither
    // is what a purchase is signed against.
    //
    // They are NOT in the same scale, which is the trap here: a listing's
    // price is already in base units -- 200000 for 0.2 USDC -- while the
    // item's headline price is in token units -- 0.2. Scaling both multiplied
    // every listed item by a million and put $200000 on a twenty-cent card.
    let pay_token = evm_address(
        listing
            .and_then(|value| value.get("payToken"))
            .or_else(|| entry.get("paymentToken")),
    )
    .unwrap_or_else(|| RUNTIME_CUSTODY_NATIVE_PAY_TOKEN.to_string());
    let decimals = pay_tokens
        .iter()
        .find(|(address, _)| address == &pay_token)
        .map(|(_, decimals)| *decimals);
    let price = listing
        .and_then(|value| value.get("price"))
        .and_then(whole_digits)
        .or_else(|| {
            entry
                .get("price")
                .and_then(|value| base_units(value, decimals))
        })
        .unwrap_or_else(|| "0".to_string());
    Some(MarketCatalogItem {
        ledger,
        token_id,
        operative: evm_address(operative.and_then(|value| value.get("address")))
            .unwrap_or_default(),
        seller_address: evm_address(
            listing
                .and_then(|value| value.get("seller"))
                .or_else(|| operative.and_then(|value| value.get("owner"))),
        )
        .unwrap_or_default(),
        content_access_id,
        content_cid: ipfs_cid(
            metadata
                .and_then(|value| value.get("media"))
                .and_then(|value| value.get("uri")),
        )
        .unwrap_or_default(),
        metadata_cid: ipfs_directory_cid(entry.get("tokenURI")).unwrap_or_default(),
        display_name: bounded_directory_text(entry.get("name")),
        content_category: bounded_directory_text(
            metadata
                .and_then(|value| value.get("media"))
                .and_then(|value| value.get("contentType")),
        )
        .to_ascii_lowercase(),
        price,
        pay_token,
        quantity: listing
            .and_then(|value| value.get("quantity"))
            .and_then(whole_number)
            .unwrap_or_default(),
        op_type: operative
            .and_then(|value| value.get("opType"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default() as u16,
        views: entry
            .get("views")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        // The index counts in milliseconds and this Home counts in seconds.
        published_at: entry
            .get("createdAt")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value / 1000)
            .unwrap_or_default(),
        // Filled by the join against this Home's own listings. An item it
        // holds nothing for is one to buy.
        mint_id: String::new(),
        access_state: "available".to_string(),
    })
}

/// A token amount the index states in token units, as the base units every
/// other price on this Home is carried in.
///
/// The index answers with a JSON number, so the digits are taken from its
/// own text rather than through a float: `0.1` at six decimals is exactly
/// `100000`, and arriving there by multiplication is how a price becomes
/// `99999.99999999999`.
///
/// Refused when this Home does not know the token's decimals. A price whose
/// scale is unknown is not a price, and a guess here is a card that lies
/// about money.
fn base_units(value: &serde_json::Value, decimals: Option<u8>) -> Option<String> {
    let decimals = usize::from(decimals?);
    let text = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_string(),
        _ => return None,
    };
    // A JSON number may arrive in exponent form -- 0.000001 is `1e-6` once
    // serialised -- so it is written out before its digits are read.
    let text = expand_exponent(&text)?;
    if text.is_empty()
        || !text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return None;
    }
    let (whole, fraction) = match text.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (text.as_str(), ""),
    };
    if fraction.len() > decimals {
        // More precision than the token has. Refused rather than rounded:
        // rounding a price is changing it.
        return None;
    }
    let mut digits = String::with_capacity(whole.len() + decimals);
    digits.push_str(if whole.is_empty() { "0" } else { whole });
    digits.push_str(fraction);
    digits.extend(std::iter::repeat_n('0', decimals - fraction.len()));
    let trimmed = digits.trim_start_matches('0');
    Some(if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    })
}

/// `1e-6` as `0.000001`, and `1.5e3` as `1500`. A plain decimal is returned
/// unchanged.
///
/// Digits are moved, never multiplied: the point of reading a price as text
/// is that no step of it goes through a float.
fn expand_exponent(text: &str) -> Option<String> {
    let Some((mantissa, exponent)) = text.split_once(['e', 'E']) else {
        return Some(text.to_string());
    };
    let exponent: i32 = exponent.parse().ok()?;
    let (whole, fraction) = match mantissa.split_once('.') {
        Some((whole, fraction)) => (whole.to_string(), fraction.to_string()),
        None => (mantissa.to_string(), String::new()),
    };
    let digits = format!("{whole}{fraction}");
    if !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Where the point sits once the exponent has moved it.
    let point = i32::try_from(whole.len()).ok()? + exponent;
    if point <= 0 {
        let zeros = usize::try_from(-point).ok()?;
        return Some(format!("0.{}{digits}", "0".repeat(zeros)));
    }
    let point = usize::try_from(point).ok()?;
    if point >= digits.len() {
        return Some(format!("{digits}{}", "0".repeat(point - digits.len())));
    }
    Some(format!("{}.{}", &digits[..point], &digits[point..]))
}

/// A value already in base units, as the digits it is written with.
///
/// Refused when it is not a whole number: base units have no fraction, so a
/// fractional one means the field is not what it is taken to be, and reading
/// it anyway would be how a price quietly changes scale.
fn whole_digits(value: &serde_json::Value) -> Option<String> {
    let text = match value {
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::String(text) => text.trim().to_string(),
        _ => return None,
    };
    let text = expand_exponent(&text)?;
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let trimmed = text.trim_start_matches('0');
    Some(if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    })
}

/// A whole number the index may answer as either a number or a string.
fn whole_number(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(number) => number.as_u64(),
        serde_json::Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn evm_address(value: Option<&serde_json::Value>) -> Option<String> {
    let address = value
        .and_then(serde_json::Value::as_str)?
        .trim()
        .to_ascii_lowercase();
    is_evm_address_lowercase(&address).then_some(address)
}

/// A uint256 as this Home writes one: `0x`, then hex with no leading zeros.
fn hex_quantity(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value.and_then(serde_json::Value::as_str)?.trim();
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))?;
    if digits.is_empty()
        || digits.len() > 64
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let trimmed = digits.trim_start_matches('0');
    Some(format!(
        "0x{}",
        if trimmed.is_empty() {
            "0".to_string()
        } else {
            trimmed.to_ascii_lowercase()
        }
    ))
}

/// The content access id, which the index writes without its `0x`.
fn content_access_id(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value
        .and_then(serde_json::Value::as_str)?
        .trim()
        .trim_start_matches("0x")
        .to_ascii_lowercase();
    (text.len() == 32 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| format!("0x{text}"))
}

/// `ipfs://<cid>` as the CID alone.
fn ipfs_cid(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value.and_then(serde_json::Value::as_str)?.trim();
    let cid = text.strip_prefix("ipfs://")?.trim_end_matches('/');
    is_plausible_cid(cid).then(|| cid.to_string())
}

/// `ipfs://<cid>/metadata.json` as the directory it names. The token URI
/// points at the document; what this Home asks its own node for is the
/// directory that holds it.
fn ipfs_directory_cid(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value.and_then(serde_json::Value::as_str)?.trim();
    let path = text.strip_prefix("ipfs://")?;
    let cid = path.split('/').next()?;
    is_plausible_cid(cid).then(|| cid.to_string())
}

#[derive(Debug, Clone, Serialize)]
pub(in crate::api::gateway) struct MarketCatalogItem {
    pub ledger: String,
    #[serde(rename = "tokenId")]
    pub token_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub operative: String,
    #[serde(rename = "sellerAddress", skip_serializing_if = "String::is_empty")]
    pub seller_address: String,
    #[serde(rename = "contentAccessId")]
    pub content_access_id: String,
    #[serde(rename = "contentCid", skip_serializing_if = "String::is_empty")]
    pub content_cid: String,
    #[serde(rename = "metadataCid", skip_serializing_if = "String::is_empty")]
    pub metadata_cid: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "contentCategory")]
    pub content_category: String,
    pub price: String,
    #[serde(rename = "payToken")]
    pub pay_token: String,
    pub quantity: u64,
    #[serde(rename = "opType")]
    pub op_type: u16,
    pub views: u64,
    #[serde(rename = "publishedAt")]
    pub published_at: u64,
    /// This Home's own listing for the same asset, when it has one. Its
    /// absence is what makes an item something to buy rather than to open.
    #[serde(rename = "mintId", skip_serializing_if = "String::is_empty")]
    pub mint_id: String,
    /// What this person may do with it: `creator`, `purchased`, or
    /// `available` when this Home holds no listing for it at all.
    #[serde(rename = "accessState")]
    pub access_state: String,
}

/// Every channel the index knows, newest page first.
///
/// No `access` filter, unlike the Creator's picker: Shops is a place to look
/// at what others publish, not a list of what this account may publish into.
/// The two questions differ, so they are asked differently even though the
/// same call answers both.
pub(in crate::api::gateway) async fn fetch_market_channels(
    data_dir: &FsPath,
) -> anyhow::Result<Vec<MarketChannelEntry>> {
    MARKET_DIRECTORY.validate_source(data_dir)?;
    let payload = MARKET_DIRECTORY
        .post_graphql(
            MARKET_CHANNELS_QUERY,
            serde_json::json!({
                "query": {},
                "filters": { "limit": MARKET_CHANNELS_PAGE_LIMIT, "offset": 0 },
            }),
        )
        .await?;
    market_channels_from_directory_payload(&payload)
}

/// What a shop card shows and nothing else. `itemsCount` and `floorPrice` are
/// the index's own arithmetic over chain state: useful on a card, never a term
/// of a sale, which is read from the chain when one is made.
pub(in crate::api::gateway) const MARKET_CHANNELS_QUERY: &str = "query FetchChannels($query: ChannelQueryInput, $filters: FilterPaginationInput) { result: fetchChannels(query: $query, filters: $filters) { total data { address name description categories itemsCount image coverImage } } }";

pub(in crate::api::gateway) const MARKET_CHANNELS_PAGE_LIMIT: u32 = 50;

pub(in crate::api::gateway) fn market_channels_from_directory_payload(
    payload: &serde_json::Value,
) -> anyhow::Result<Vec<MarketChannelEntry>> {
    let entries = MARKET_DIRECTORY.rows(payload, "result")?;
    let mut channels = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(address) = entry
            .get("address")
            .and_then(serde_json::Value::as_str)
            .map(|address| address.trim().to_ascii_lowercase())
        else {
            continue;
        };
        // A row this Runtime cannot address is not a channel it can show.
        if !is_evm_address_lowercase(&address) {
            continue;
        }
        channels.push(MarketChannelEntry {
            image_cid: directory_image_cid(entry.get("image"))
                .or_else(|| directory_image_cid(entry.get("coverImage")))
                .unwrap_or_default(),
            address,
            name: bounded_directory_text(entry.get("name")),
            description: bounded_directory_text(entry.get("description")),
            categories: entry
                .get("categories")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| value.as_str())
                        .map(|value| bounded_directory_text(Some(&serde_json::json!(value))))
                        .filter(|value| !value.is_empty())
                        .take(MARKET_DIRECTORY_MAX_CATEGORIES)
                        .collect()
                })
                .unwrap_or_default(),
            items_count: entry
                .get("itemsCount")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or_default(),
        });
    }
    Ok(channels)
}

/// A channel picture, but only when the index answered with something this
/// Home can fetch for itself.
///
/// The index's own URL fields are unusable and must not be followed. Live
/// answers include `imageURL: "http://10.132.0.5:8080/ipfs/initials:W"` -- an
/// RFC1918 address, their internal gateway leaked into a public API -- and
/// `coverImageURL: "thumbnail:1789921246918.jpg"`, which is not a URL at all
/// but an instruction to their thumbnail service. Following either would have
/// this Home fetching from a private network or depending on someone else's
/// image host.
///
/// The `image` and `coverImage` fields do carry a plain CID when a channel has
/// a picture, and a CID is content this Home already knows how to fetch. So
/// only a CID is accepted: `""`, `initials:W`, anything with a scheme, a slash
/// or a colon is not one, and a channel keeps its glyph instead.
fn directory_image_cid(value: Option<&serde_json::Value>) -> Option<String> {
    let text = value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if !is_plausible_cid(text) {
        return None;
    }
    Some(text.to_string())
}

/// CIDv0 (`Qm…`, base58) or CIDv1 (`b…`, base32). Deliberately narrow: this
/// decides what this Home will ask its own IPFS node to fetch, so a value that
/// is merely string-like does not qualify.
fn is_plausible_cid(value: &str) -> bool {
    if value.len() < 46 || value.len() > 96 {
        return false;
    }
    if value.starts_with("Qm") {
        return value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                && byte != b'0'
                && byte != b'I'
                && byte != b'O'
                && byte != b'l'
        });
    }
    if value.starts_with('b') {
        return value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit());
    }
    false
}

/// Text from a source outside this Home, cut to a length a card can hold.
///
/// Bounded here rather than in the page: a directory is free to answer with a
/// megabyte of prose, and the first thing that should refuse it is the half
/// that decided to ask.
fn bounded_directory_text(value: Option<&serde_json::Value>) -> String {
    let text = value
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim();
    if text.chars().count() <= MARKET_DIRECTORY_MAX_TEXT_CHARS {
        return text.to_string();
    }
    text.chars().take(MARKET_DIRECTORY_MAX_TEXT_CHARS).collect()
}

const MARKET_DIRECTORY_MAX_TEXT_CHARS: usize = 256;
const MARKET_DIRECTORY_MAX_CATEGORIES: usize = 8;

/// `0x` + 40 lowercase hex. Local to this module: it screens an address the
/// index reported, not a wallet this Runtime will act on.
fn is_evm_address_lowercase(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[derive(Debug, Clone, Serialize)]
pub(in crate::api::gateway) struct MarketChannelEntry {
    pub address: String,
    /// The channel's picture, as a CID.
    ///
    /// Served to the page, which asks this Home's own `/ipfs/:cid` for it --
    /// the route that already fetches from the local node and caches to disk,
    /// and the one Explore's covers will use too. A picture is content this
    /// Home serves, not a second content path built for one surface.
    ///
    /// Only a CID reaches here, never a URL the index supplied: what this Home
    /// fetches stays a CID it resolves through its own node.
    #[serde(rename = "imageCid", skip_serializing_if = "String::is_empty")]
    pub image_cid: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(rename = "itemsCount")]
    pub items_count: u64,
}

#[derive(Debug, Serialize)]
pub(in crate::api::gateway) struct MarketChannelsResponse {
    #[serde(rename = "asOf")]
    pub as_of: u64,
    pub stale: bool,
    pub unavailable: bool,
    #[serde(rename = "needsApproval")]
    pub needs_approval: bool,
    pub channels: Vec<MarketChannelEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}
