use super::*;
use serde_json::json;

const SHARED_CONTENT_CHECK_SCHEMA: &str = "elastos.home.shared-content.check/v1";
const SHARED_CONTENT_RESULT_SCHEMA: &str = "elastos.home.shared-content.result/v1";
/// How long one `check` answer may be redeemed by `get`.
const SHARED_CONTENT_CHECK_TTL_SECS: u64 = 120;
/// Open checks kept per Runtime; the oldest goes first.
const MAX_OPEN_SHARED_CONTENT_CHECKS: usize = 64;
/// The bounded wait for the sender's Home to answer one `get`.
const SHARED_CONTENT_REPLY_WAIT: std::time::Duration = std::time::Duration::from_secs(300);
/// Longest `elastos://` link Home checks, in bytes.
const MAX_SHARED_CONTENT_URI_BYTES: usize = 256;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeSharedContentCheckRequest {
    schema: String,
    app_token: String,
    uri: String,
    #[serde(default)]
    source: Option<HomeSharedContentSource>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct HomeSharedContentSource {
    conversation_id: String,
    message_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HomeSharedContentGetRequest {
    check_id: String,
}

/// What one `check` answer lets `get` do, and for whom.
#[derive(Debug, Clone)]
struct SharedContentCheck {
    principal_id: String,
    session_id: String,
    app_id: String,
    cid: String,
    peer_did: String,
    message: HomeSharedContentSource,
    expires_at: u64,
}

fn open_checks() -> &'static std::sync::Mutex<Vec<(String, SharedContentCheck)>> {
    static CHECKS: std::sync::Mutex<Vec<(String, SharedContentCheck)>> =
        std::sync::Mutex::new(Vec::new());
    &CHECKS
}

fn remember_check(check: SharedContentCheck, now: u64) -> String {
    let check_id = hex::encode(rand::random::<[u8; 16]>());
    let mut checks = open_checks().lock().unwrap_or_else(|err| err.into_inner());
    checks.retain(|(_, open)| open.expires_at > now);
    if checks.len() >= MAX_OPEN_SHARED_CONTENT_CHECKS {
        checks.remove(0);
    }
    checks.push((check_id.clone(), check));
    check_id
}

/// Redeems `check_id` once, only for the Home session that asked.
fn redeem_check(
    check_id: &str,
    home: &HomeLaunchTokenContext,
    now: u64,
) -> Option<SharedContentCheck> {
    let mut checks = open_checks().lock().unwrap_or_else(|err| err.into_inner());
    let index = checks.iter().position(|(id, _)| id == check_id)?;
    let (_, check) = checks.remove(index);
    (check.expires_at > now
        && check.principal_id == home.principal_id
        && check.session_id == home.session_id)
        .then_some(check)
}

/// Moves one open check `secs` closer to its expiry, as if that much time had
/// passed since `check` issued it.
#[cfg(test)]
pub(super) fn age_shared_content_check_for_test(check_id: &str, secs: u64) {
    let mut checks = open_checks().lock().unwrap_or_else(|err| err.into_inner());
    for (id, check) in checks.iter_mut() {
        if id == check_id {
            check.expires_at -= secs;
        }
    }
}

/// One `get` at a time per Home; dropping the guard (also on cancel) frees it.
struct GetInFlight(std::path::PathBuf);

impl GetInFlight {
    fn begin(data_dir: &std::path::Path) -> Option<Self> {
        let mut busy = gets_in_flight()
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        busy.insert(data_dir.to_path_buf())
            .then(|| Self(data_dir.to_path_buf()))
    }
}

impl Drop for GetInFlight {
    fn drop(&mut self) {
        gets_in_flight()
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(&self.0);
    }
}

fn gets_in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>> {
    static BUSY: std::sync::LazyLock<
        std::sync::Mutex<std::collections::HashSet<std::path::PathBuf>>,
    > = std::sync::LazyLock::new(Default::default);
    &BUSY
}

/// The CID an exact `elastos://<cid>` link names: a CID that parses and
/// re-serialises to exactly the host, with no path, query or fragment.
fn exact_shared_content_cid(uri: &str) -> anyhow::Result<String> {
    let cid = uri
        .strip_prefix("elastos://")
        .filter(|_| uri.len() <= MAX_SHARED_CONTENT_URI_BYTES)
        .ok_or_else(|| anyhow::anyhow!("invalid shared file link"))?;
    let parsed =
        cid::Cid::try_from(cid).map_err(|_| anyhow::anyhow!("invalid shared file link"))?;
    anyhow::ensure!(parsed.to_string() == cid, "invalid shared file link");
    Ok(cid.to_string())
}

/// Whether `body` carries `uri` as one link token (trailing punctuation as
/// Chat trims it).
fn body_cites(body: &str, uri: &str) -> bool {
    body.split_whitespace().any(|word| {
        word.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}', '\'', '"']) == uri
    })
}

/// The size a Chat file post states (`name · size · elastos://<cid>`), in
/// bytes as the sender's Home formatted it.
fn stated_post_size(body: &str, uri: &str) -> Option<u64> {
    let mut parts = body.rsplitn(3, " · ");
    if parts.next()? != uri {
        return None;
    }
    let size = parts.next()?;
    parts.next()?;
    if let Some(bytes) = size.strip_suffix(" bytes") {
        return bytes.parse().ok();
    }
    let (number, unit) = size.split_once(' ')?;
    let scale = match unit {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let value: f64 = number.parse().ok()?;
    (value.is_finite() && value >= 0.0).then_some((value * scale) as u64)
}

/// The size format Chat shows (`format_bytes` in the Chat app).
pub(super) fn format_shared_file_size(size_bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let size = size_bytes as f64;
    if size >= GB {
        format!("{:.1} GB", size / GB)
    } else if size >= MB {
        format!("{:.1} MB", size / MB)
    } else if size >= KB {
        format!("{:.1} KB", size / KB)
    } else {
        format!("{size_bytes} bytes")
    }
}

/// An incoming message cited by Chat, as this principal's stores hold it.
struct CitedMessage {
    sender_display_name: String,
    /// The accepted contact's delivery device, when the sender is one.
    contact: Option<(String, String)>,
    body: String,
}

fn cited_message(
    state: &GatewayState,
    context: &HomeLaunchTokenContext,
    source: &HomeSharedContentSource,
) -> anyhow::Result<CitedMessage> {
    let service = state
        .collaboration_discovery_service
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Chat is not configured"))?;
    let authority = gateway_home_system::load_configured_contact_authority_for_context(
        &state.data_dir,
        context,
        Some(service),
    )?
    .ok_or_else(|| anyhow::anyhow!("Chat requires a signed Profile"))?;
    let contacts = authority.store.snapshot()?;
    let local_profile_did = authority.profile.document().profile_did.as_str();
    let community = state
        .collaboration_chat_product_port
        .as_ref()
        .map(|port| port.history_scope())
        .filter(|(_, conversation_id)| *conversation_id == source.conversation_id);
    if let Some((network_id, conversation_id)) = community {
        let row = crate::room_service::collaboration_text_row(
            &state.data_dir,
            network_id,
            conversation_id,
            &source.message_id,
        )?
        .ok_or_else(|| anyhow::anyhow!("message not found"))?;
        anyhow::ensure!(
            row.sender_profile_did != local_profile_did,
            "only a message from someone else can share a file"
        );
        let contact = contacts
            .contacts()
            .iter()
            .find(|contact| contact.remote_profile_did() == row.sender_profile_did)
            .map(|contact| {
                (
                    contact.remote_profile_did().to_string(),
                    contact.remote_presence_device_did().to_string(),
                )
            });
        return Ok(CitedMessage {
            sender_display_name: row.sender_display_name,
            contact,
            body: row.body,
        });
    }
    let messages = service
        .direct_message_service()
        .message_summaries(
            authority.store.as_ref(),
            &authority.profile,
            &source.conversation_id,
            now_ts(),
        )
        .map_err(|_| anyhow::anyhow!("message not found"))?;
    let message = messages
        .into_iter()
        .find(|message| message.message_id == source.message_id)
        .ok_or_else(|| anyhow::anyhow!("message not found"))?;
    anyhow::ensure!(
        message.direction == "incoming",
        "only a message from someone else can share a file"
    );
    let contact = contacts
        .contacts()
        .iter()
        .find(|contact| contact.conversation_id() == source.conversation_id);
    Ok(CitedMessage {
        sender_display_name: contact
            .map(|contact| contact.remote_display_name().to_string())
            .unwrap_or_default(),
        contact: contact.map(|contact| {
            (
                contact.remote_profile_did().to_string(),
                contact.remote_presence_device_did().to_string(),
            )
        }),
        body: message.text,
    })
}

/// Home asks what an `elastos://` link opened from an app would do. The check
/// is local only: it reads the cited message, this Home's receipts, contacts
/// and presence, and touches no network. Presence is a hint, not a gate.
pub(super) async fn home_shared_content_check(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeSharedContentCheckRequest>,
) -> Response {
    if input.schema != SHARED_CONTENT_CHECK_SCHEMA {
        return (
            StatusCode::BAD_REQUEST,
            "unsupported shared-content request schema",
        )
            .into_response();
    }
    let (home, app) = match require_home_token_context(&state.data_dir, &headers).and_then(|home| {
        home_external_link_app(&state.data_dir, &headers, &input.app_token).map(|app| (home, app))
    }) {
        Ok(found) => found,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let source_allowed = match app.id.as_str() {
        CHAT_ROOM_CAPSULE_ID => true,
        DOCUMENTS_CAPSULE_ID => input.source.is_none(),
        _ => false,
    };
    if !source_allowed {
        return (StatusCode::FORBIDDEN, "this app cannot open shared files").into_response();
    }
    let cid = match exact_shared_content_cid(&input.uri) {
        Ok(cid) => cid,
        Err(err) => return (StatusCode::BAD_REQUEST, err.to_string()).into_response(),
    };
    let Some(registry) = state.provider_registry.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "content is unavailable").into_response();
    };
    let uri = format!("elastos://{cid}");
    let reply = |state_name: &str, extra: serde_json::Value| {
        let mut body = json!({
            "schema": SHARED_CONTENT_RESULT_SCHEMA,
            "cid": cid,
            "uri": uri,
            "state": state_name,
        });
        if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            body.extend(extra.clone());
        }
        Json(body).into_response()
    };
    let Some(source) = input.source else {
        let here = crate::content::content_pinned_here(&registry, &cid).await;
        return reply(if here { "local" } else { "not_here" }, json!({}));
    };
    let cited = {
        let (state, home, source) = (state.clone(), home.clone(), source.clone());
        tokio::task::spawn_blocking(move || cited_message(&state, &home, &source)).await
    };
    let cited = match cited {
        Ok(Ok(cited)) if body_cites(&cited.body, &uri) => cited,
        Ok(Ok(_)) => {
            return (
                StatusCode::BAD_REQUEST,
                "the message does not share this file",
            )
                .into_response()
        }
        Ok(Err(err)) => return (StatusCode::NOT_FOUND, err.to_string()).into_response(),
        Err(_) => return (StatusCode::INTERNAL_SERVER_ERROR, "check failed").into_response(),
    };
    let mut extra = json!({ "sender_display_name": cited.sender_display_name });
    if let Some(size) = stated_post_size(&cited.body, &uri) {
        extra["size_bytes"] = json!(size);
    }
    if crate::content::content_pinned_here(&registry, &cid).await {
        return reply("local", extra);
    }
    let Some((profile_did, peer_did)) = cited.contact else {
        return reply("not_contact", extra);
    };
    let now = now_ts();
    let online = match state.collaboration_presence_product_port.as_ref() {
        Some(port) => port
            .authorized_snapshot(now)
            .await
            .ok()
            .is_some_and(|snapshot| {
                gateway_home_system::profile_reachable(&snapshot, &profile_did, now)
            }),
        None => false,
    };
    extra["check_id"] = json!(remember_check(
        SharedContentCheck {
            principal_id: home.principal_id.clone(),
            session_id: home.session_id.clone(),
            app_id: app.id,
            cid: cid.clone(),
            peer_did,
            message: source,
            expires_at: now + SHARED_CONTENT_CHECK_TTL_SECS,
        },
        now,
    ));
    reply(
        if online {
            "available"
        } else {
            "sender_offline"
        },
        extra,
    )
}

/// Home gets the file one `check` named from the sender's Home and keeps it
/// only when its bytes match the link. Home-shell authority only.
pub(super) async fn home_shared_content_get(
    State(state): State<GatewayState>,
    headers: HeaderMap,
    Json(input): Json<HomeSharedContentGetRequest>,
) -> Response {
    let home = match require_home_token_context(&state.data_dir, &headers) {
        Ok(home) => home,
        Err(err) => return (StatusCode::FORBIDDEN, err.to_string()).into_response(),
    };
    let Some(registry) = state.provider_registry.clone() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "content is unavailable").into_response();
    };
    let Some(_in_flight) = GetInFlight::begin(&state.data_dir) else {
        return (StatusCode::CONFLICT, "Another file is still being fetched.").into_response();
    };
    let Some(check) = redeem_check(&input.check_id, &home, now_ts()) else {
        return (StatusCode::GONE, "this check has expired or was used").into_response();
    };
    tracing::debug!(
        app = %check.app_id,
        conversation = %check.message.conversation_id,
        message = %check.message.message_id,
        "shared content get"
    );
    let outcome = if crate::content::content_pinned_here(&registry, &check.cid).await {
        Ok(crate::content::SharedContentGet::Local)
    } else {
        crate::content::get_shared_content_from_peer(
            &registry,
            &check.peer_did,
            &check.cid,
            "content.bin",
            SHARED_CONTENT_REPLY_WAIT,
        )
        .await
    };
    match outcome {
        Ok(outcome) => Json(json!({ "state": outcome.as_str() })).into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi";
    const V0: &str = "QmYwAPJzv5CZsnA625s3Xf2nemtYgPpHdWEz79ojWnPbdG";

    #[test]
    fn only_exact_cid_links_are_checked() {
        assert_eq!(
            exact_shared_content_cid(&format!("elastos://{V1}")).unwrap(),
            V1
        );
        assert_eq!(
            exact_shared_content_cid(&format!("elastos://{V0}")).unwrap(),
            V0
        );
        for refused in [
            format!("elastos://{V1}/a"),
            format!("elastos://{V1}?x"),
            format!("elastos://{V1}#x"),
            format!("elastos://{}", V1.to_uppercase()),
            format!("elastos://{}", &V0[..45]),
            "elastos://peer/invite?token=x".to_string(),
            format!("https://{V1}"),
            format!(" elastos://{V1}"),
        ] {
            assert!(exact_shared_content_cid(&refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_post_cites_its_link_and_states_its_size() {
        let uri = format!("elastos://{V1}");
        let body = format!("notes.txt · 1.0 MB · {uri}");
        assert!(body_cites(&body, &uri));
        assert!(body_cites(&format!("see {uri}."), &uri));
        assert!(!body_cites(&format!("see {uri}/a"), &uri));
        assert_eq!(stated_post_size(&body, &uri), Some(1024 * 1024));
        assert_eq!(
            stated_post_size(&format!("a · b · 30 bytes · {uri}"), &uri),
            Some(30)
        );
        assert_eq!(stated_post_size(&format!("see {uri}"), &uri), None);
        assert_eq!(format_shared_file_size(30), "30 bytes");
        assert_eq!(format_shared_file_size(3_565_158), "3.4 MB");
    }

    #[test]
    fn a_check_is_redeemed_once_and_only_by_its_home_session() {
        let home = HomeLaunchTokenContext {
            principal_id: "principal".into(),
            proof_binding_id: None,
            session_id: "session".into(),
            grant_id: String::new(),
        };
        let check = |expires_at| SharedContentCheck {
            principal_id: "principal".into(),
            session_id: "session".into(),
            app_id: CHAT_ROOM_CAPSULE_ID.into(),
            cid: V1.into(),
            peer_did: "did:key:z6Mkpeer".into(),
            message: HomeSharedContentSource {
                conversation_id: "c".into(),
                message_id: "m".into(),
            },
            expires_at,
        };
        let id = remember_check(check(200), 100);
        assert_eq!(redeem_check(&id, &home, 150).unwrap().cid, V1);
        assert!(redeem_check(&id, &home, 150).is_none(), "single use");
        let other = HomeLaunchTokenContext {
            session_id: "other".into(),
            ..home.clone()
        };
        let id = remember_check(check(200), 100);
        assert!(redeem_check(&id, &other, 150).is_none(), "another session");
        assert!(
            redeem_check(&id, &home, 150).is_none(),
            "consumed by the attempt"
        );
        let id = remember_check(check(200), 100);
        assert!(redeem_check(&id, &home, 200).is_none(), "expired");
    }

    #[test]
    fn one_get_runs_at_a_time_per_home() {
        let dir = tempfile::tempdir().unwrap();
        let first = GetInFlight::begin(dir.path()).unwrap();
        assert!(GetInFlight::begin(dir.path()).is_none());
        drop(first);
        assert!(GetInFlight::begin(dir.path()).is_some());
    }
}
