//! The channel directory the creator page offers, and the consent that lets it
//! be read.
//!
//! A creator mints into a channel, and until now there was exactly one: the
//! configured ledger. Offering a choice needs a list, and no list of "channels
//! I may mint on" can be derived locally -- a public channel is mintable by
//! anyone and a private one by whoever holds its role, so the set is neither
//! bounded nor discoverable from this Home alone.
//!
//! So this reads an external directory, under the same terms the Wallet reads
//! market prices: off until the person approves it through the Inbox, the
//! approval recorded with who gave it and when, every fetch audited, and a
//! failure degrading to a note rather than a refusal.
//!
//! What it is NOT is authority. The directory says what to OFFER; the chain
//! decides what is PERMITTED, and the chosen channel is verified before a mint
//! is signed. A directory that is empty, stale, unreachable or wrong costs a
//! convenience, never a capability -- which is also why the configured channel
//! and a typed address are always in the picker.

use super::*;

/// Cached per process, like the price cache: several page loads in a row are
/// one question, and a creator's channels do not change minute to minute.
static CREATOR_CHANNELS_CACHE: OnceLock<tokio::sync::Mutex<Option<CreatorChannelsCache>>> =
    OnceLock::new();

struct CreatorChannelsCache {
    as_of: u64,
    creator: String,
    channels: Vec<CreatorChannelEntry>,
}

pub(in crate::api::gateway) async fn creator_channels(
    State(state): State<GatewayState>,
    headers: HeaderMap,
) -> Response {
    let context = match require_home_launch_token_for_any_context(
        &state.data_dir,
        &headers,
        &[CREATOR_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(context) => context,
        Err(err) => return system_error_response(err),
    };
    let authority = match require_app_runtime_wallet_authority(
        &state.data_dir,
        &headers,
        &[CREATOR_CAPSULE_ID, SYSTEM_CAPSULE_ID],
    ) {
        Ok(authority) => authority,
        Err(err) => return system_error_response(err),
    };
    // The account the mint would be signed by -- not one the page names -- and
    // what a sale may be priced in. The tokens are local configuration, so
    // they answer even when the directory is switched off: the currency the
    // form offers must never depend on an external service being reachable.
    let (creator, pay_tokens) = match runtime_custody_creator_choices(&state, &authority).await {
        Ok(choices) => choices,
        Err(err) => {
            tracing::debug!(error = %err, "creator channels: mint source unavailable");
            (None, Vec::new())
        }
    };
    let creator = creator.unwrap_or_default();
    match creator_channels_response(&state.data_dir, &creator, Some(&context)).await {
        Ok(response) => {
            // Asking is what raises the approval, so a page that has never been
            // opened does not leave an Inbox item nobody asked for.
            if response.needs_approval {
                let _ = upsert_creator_channels_http_request(&state.data_dir, &context, now_ts());
            }
            let mut answer = serde_json::to_value(&response).unwrap_or_default();
            if let Some(object) = answer.as_object_mut() {
                // The account itself is NOT returned. The page needs channels
                // to offer, not the Wallet's default address, and a capsule
                // that never had to know it should not learn it to fill a
                // picker. It is resolved and used entirely inside the Runtime.
                object.insert("payTokens".to_string(), serde_json::json!(pay_tokens));
            }
            Json(answer).into_response()
        }
        Err(err) => system_error_response(err),
    }
}

pub(in crate::api::gateway) async fn creator_channels_response(
    data_dir: &FsPath,
    creator: &str,
    context: Option<&HomeLaunchTokenContext>,
) -> anyhow::Result<CreatorChannelsResponse> {
    let now = now_ts();
    let cache = CREATOR_CHANNELS_CACHE.get_or_init(|| tokio::sync::Mutex::new(None));
    {
        let guard = cache.lock().await;
        if let Some(current) = guard.as_ref() {
            // Keyed by creator as well as age: a different account is a
            // different question, not a stale answer to the same one.
            if current.creator == creator
                && now.saturating_sub(current.as_of) <= CREATOR_CHANNELS_CACHE_TTL_SECS
            {
                return Ok(CreatorChannelsResponse {
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

    let fetch_request_id = format!("{CREATOR_CHANNELS_HTTP_REQUEST_ID}:fetch:{now}");
    if let Some(context) = context {
        append_creator_channels_fetch_audit(
            data_dir,
            context,
            &fetch_request_id,
            "requested",
            "Creator requested approved external HTTP channel-directory fetch",
        )?;
    }
    match fetch_creator_channels(data_dir, creator).await {
        Ok(channels) => {
            let response = CreatorChannelsResponse {
                as_of: now,
                stale: false,
                unavailable: false,
                needs_approval: false,
                channels,
                note: None,
            };
            *cache.lock().await = Some(CreatorChannelsCache {
                as_of: response.as_of,
                creator: creator.to_string(),
                channels: response.channels.clone(),
            });
            if let Some(context) = context {
                append_creator_channels_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "completed",
                    "Creator completed approved external HTTP channel-directory fetch",
                )?;
            }
            Ok(response)
        }
        Err(err) => {
            if let Some(context) = context {
                let _ = append_creator_channels_fetch_audit(
                    data_dir,
                    context,
                    &fetch_request_id,
                    "failed",
                    "Creator external HTTP channel-directory fetch failed or was blocked",
                );
            }
            let note = err.to_string();
            let needs_approval = creator_channels_note_should_request_approval(&note);
            let guard = cache.lock().await;
            // A stale list for this creator is better than none: these
            // addresses are checked on chain before use, so age costs nothing
            // but freshness.
            if let Some(current) = guard.as_ref().filter(|cached| cached.creator == creator) {
                Ok(CreatorChannelsResponse {
                    as_of: current.as_of,
                    stale: true,
                    unavailable: false,
                    needs_approval,
                    channels: current.channels.clone(),
                    note: Some(note),
                })
            } else {
                Ok(CreatorChannelsResponse {
                    as_of: now,
                    stale: true,
                    unavailable: true,
                    needs_approval,
                    channels: Vec::new(),
                    note: Some(note),
                })
            }
        }
    }
}

/// Reads the directory. Refuses before any request leaves this Home unless the
/// source is configured and the person has approved it.
///
/// The question asked is `fetchChannels(query: { access: "mint:0x…" })`, which
/// the directory answers with the channels that account may publish into.
///
/// Not `creator`: a public channel is mintable by anyone, so the set a creator
/// can use is wider than the set they made -- on this Home the configured
/// channel is one the creator did not create, and a `creator` filter returns
/// nothing at all for them.
///
/// Not `user:` either, though that prefix also answers. It is the wider set of
/// channels the account can reach, which includes ones it may not publish
/// into; offering those would put a channel in the picker that refuses the
/// mint. `mint:` is the question this surface is actually asking.
pub(in crate::api::gateway) async fn fetch_creator_channels(
    data_dir: &FsPath,
    creator: &str,
) -> anyhow::Result<Vec<CreatorChannelEntry>> {
    CREATOR_CHANNEL_DIRECTORY.validate_source(data_dir)?;
    if !is_evm_address_lowercase(creator) {
        anyhow::bail!("channel directory needs the creator's address");
    }
    let payload = CREATOR_CHANNEL_DIRECTORY
        .post_graphql(
            CREATOR_CHANNELS_QUERY,
            serde_json::json!({
                "query": { "access": creator_channels_access_filter(creator) },
                "filters": { "limit": CREATOR_CHANNELS_PAGE_LIMIT, "offset": 0 },
            }),
        )
        .await?;
    creator_channels_from_directory_payload(&payload)
}

/// The directory's filter for "channels this account may publish into".
///
/// `mint:` and not `user:`: both answer, but `user:` is the wider set of
/// channels the account can reach, which includes ones it may not publish
/// into. Offering one of those would put a channel in the picker that then
/// refuses the mint.
pub(in crate::api::gateway) fn creator_channels_access_filter(creator: &str) -> String {
    format!("mint:{creator}")
}

/// Only what the picker shows. Asking for less is not only cheaper: every
/// extra field is one more thing a reader might be tempted to treat as a fact
/// about permission, which none of them are.
pub(in crate::api::gateway) const CREATOR_CHANNELS_QUERY: &str = "query FetchChannels($query: ChannelQueryInput, $filters: FilterPaginationInput) { result: fetchChannels(query: $query, filters: $filters) { total data { address name } } }";

/// One page is the whole picker. A creator with more channels than this can
/// still type an address, and a scrolling directory is not what this surface
/// is for.
pub(in crate::api::gateway) const CREATOR_CHANNELS_PAGE_LIMIT: u32 = 50;

/// Reads the directory's answer.
///
/// GraphQL reports failures inside a 200, so `errors` is checked before `data`
/// -- otherwise a refused query reads as a creator with no channels, and the
/// picker would quietly offer nothing at all rather than say what happened.
pub(in crate::api::gateway) fn creator_channels_from_directory_payload(
    payload: &serde_json::Value,
) -> anyhow::Result<Vec<CreatorChannelEntry>> {
    let entries = CREATOR_CHANNEL_DIRECTORY.rows(payload, "result")?;
    let mut channels = Vec::with_capacity(entries.len());
    for entry in entries {
        let Some(address) = entry
            .get("address")
            .and_then(serde_json::Value::as_str)
            .map(|address| address.trim().to_ascii_lowercase())
        else {
            continue;
        };
        // A row this Runtime cannot address is not a channel it can offer.
        if !is_evm_address_lowercase(&address) {
            continue;
        }
        let name = entry
            .get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        channels.push(CreatorChannelEntry {
            address,
            name,
            configured: false,
        });
    }
    Ok(channels)
}

/// This surface's directory. Its identifiers are its own: approving the
/// channel picker approves the channel picker, and nothing else this Home
/// reads from the same index.
pub(in crate::api::gateway) const CREATOR_CHANNEL_DIRECTORY: OnchainDirectory = OnchainDirectory {
    capsule_id: CREATOR_CAPSULE_ID,
    subject: "channel directory",
    request_id: CREATOR_CHANNELS_HTTP_REQUEST_ID,
    approve_action_id: CREATOR_CHANNELS_HTTP_APPROVE_ACTION_ID,
    deny_action_prefix: CREATOR_CHANNELS_HTTP_DENY_ACTION_PREFIX,
    deny_action_suffix: "onchain-graphql",
    source_env: CREATOR_CHANNELS_SOURCE_ENV,
    approved_env: CREATOR_CHANNELS_HTTP_APPROVED_ENV,
    policy_schema: CREATOR_CHANNELS_POLICY_SCHEMA,
    policy_root: CREATOR_CHANNELS_POLICY_ROOT,
    policy_file: CREATOR_CHANNELS_POLICY_FILE,
    audit_prefix: "creator.channel_directory",
    request_title: "Creator requests your channel list",
    request_body: "Creator wants approved HTTP access to an on-chain index, to list the channels you can publish into. Publishing works without it; you would type a channel address instead.",
    user_agent: "ElastOS-Creator/0.2",
};

pub(in crate::api::gateway) fn creator_channels_note_should_request_approval(note: &str) -> bool {
    CREATOR_CHANNEL_DIRECTORY.note_should_request_approval(note)
}

pub(in crate::api::gateway) fn store_creator_channels_http_policy(
    data_dir: &FsPath,
    principal_id: &str,
    approved_at: u64,
) -> anyhow::Result<()> {
    CREATOR_CHANNEL_DIRECTORY.store_policy(data_dir, principal_id, approved_at)
}

pub(in crate::api::gateway) fn upsert_creator_channels_http_request(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    created_at: u64,
) -> anyhow::Result<()> {
    CREATOR_CHANNEL_DIRECTORY.upsert_request(data_dir, context, created_at)
}

pub(in crate::api::gateway) fn append_creator_channels_policy_audit(
    data_dir: &FsPath,
    principal_id: &str,
    session_id: &str,
    result: &str,
    reason: &str,
) -> anyhow::Result<()> {
    CREATOR_CHANNEL_DIRECTORY.append_policy_audit(
        data_dir,
        principal_id,
        session_id,
        result,
        reason,
    )
}

pub(in crate::api::gateway) fn append_creator_channels_fetch_audit(
    data_dir: &FsPath,
    context: &HomeLaunchTokenContext,
    request_id: &str,
    result: &str,
    reason: &str,
) -> anyhow::Result<()> {
    CREATOR_CHANNEL_DIRECTORY.append_fetch_audit(data_dir, context, request_id, result, reason)
}

/// `0x` + 40 lowercase hex. Local to this module: it screens an address before
/// it is put in a request, not a wallet this Runtime will act on.
fn is_evm_address_lowercase(value: &str) -> bool {
    value.len() == 42
        && value.starts_with("0x")
        && value[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
