use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::api;
use crate::app;
use crate::app::Message;

const CHAT_DISCOVERY_TOPIC_GENERAL: &str = "__elastos_internal/chat-presence-v1/#general";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatPresenceAnnouncement {
    pub kind: String,
    pub room: String,
    pub did: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub nick: String,
    pub ticket: String,
    pub ts: u64,
}

pub struct ResolvedIdentity {
    pub token: String,
    pub did: String,
    pub nickname: String,
}

pub fn resolve_identity(requested_nick: &str, nick_explicit: bool) -> Result<ResolvedIdentity> {
    let token = api::acquire_capability("elastos://did/*", "execute")?;

    let did = api::provider_call(&token, "did", "get_did", &serde_json::json!({}))
        .ok()
        .and_then(|resp| {
            resp.get("data")
                .and_then(|d| d.get("did"))
                .and_then(|v| v.as_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_default();

    let nickname = if nick_explicit {
        requested_nick.to_string()
    } else {
        api::provider_call(&token, "did", "get_nickname", &serde_json::json!({}))
            .ok()
            .and_then(|resp| {
                resp.get("data")
                    .and_then(|d| d.get("nickname"))
                    .and_then(|n| n.as_str())
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_else(|| requested_nick.to_string())
    };

    Ok(ResolvedIdentity {
        token,
        did,
        nickname,
    })
}

pub fn acquire_peer_token() -> Result<String> {
    api::acquire_capability("elastos://peer/*", "execute")
}

pub fn acquire_storage_token() -> Result<String> {
    api::acquire_capability("localhost://Users/self/.AppData/LocalHost/Chat/*", "write")
}

pub fn set_nickname(identity_token: &str, nickname: &str) -> Result<()> {
    api::provider_call(
        identity_token,
        "did",
        "set_nickname",
        &serde_json::json!({"nickname": nickname}),
    )?;
    Ok(())
}

pub fn sign_message(
    identity_token: &str,
    sender_id: &str,
    ts: u64,
    content: &str,
) -> Result<Option<String>> {
    let payload_hex = app::signing_payload_hex(sender_id, ts, content);
    let response = api::provider_call(
        identity_token,
        "did",
        "sign",
        &serde_json::json!({"data": payload_hex}),
    )?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("signature"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string()))
}

pub fn verify_message(identity_token: &str, msg: &Message) -> Result<bool> {
    let signature = match &msg.signature {
        Some(signature) if !signature.is_empty() => signature,
        _ => return Ok(false),
    };
    if msg.sender_id.is_empty() {
        return Ok(false);
    }
    let payload_hex = app::signing_payload_hex(&msg.sender_id, msg.ts, &msg.content);
    let response = api::provider_call(
        identity_token,
        "did",
        "verify",
        &serde_json::json!({
            "did": msg.sender_id,
            "data": payload_hex,
            "signature": signature,
        }),
    )?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("valid"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

pub fn connect_peer(peer_token: &str, ticket: &str) -> Result<usize> {
    let response = api::provider_call(
        peer_token,
        "peer",
        "connect",
        &serde_json::json!({"ticket": ticket}),
    )?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("added"))
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .unwrap_or(0))
}

pub fn remember_peer(peer_token: &str, ticket: &str) -> Result<Vec<String>> {
    let response = api::provider_call(
        peer_token,
        "peer",
        "remember_peer",
        &serde_json::json!({"ticket": ticket}),
    )?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("added"))
        .and_then(|p| p.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

pub fn chat_discovery_topic(room: &str) -> String {
    if room == "#general" {
        CHAT_DISCOVERY_TOPIC_GENERAL.to_string()
    } else {
        format!("__elastos_internal/chat-presence-v1/{}", room)
    }
}

pub fn join_topic(peer_token: &str, topic: &str) -> Result<()> {
    join_topic_mode(peer_token, topic, None)
}

pub fn join_topic_mode(peer_token: &str, topic: &str, mode: Option<&str>) -> Result<()> {
    let mut body = serde_json::json!({"topic": topic});
    if let Some(mode) = mode.filter(|value| !value.is_empty()) {
        body["mode"] = serde_json::Value::String(mode.to_string());
    }
    api::provider_call(peer_token, "peer", "gossip_join", &body)?;
    Ok(())
}

pub fn leave_topic(peer_token: &str, topic: &str) -> Result<()> {
    api::provider_call(
        peer_token,
        "peer",
        "gossip_leave",
        &serde_json::json!({"topic": topic}),
    )?;
    Ok(())
}

pub fn list_topic_peers(peer_token: &str, topic: &str) -> Result<Vec<String>> {
    let response = api::provider_call(
        peer_token,
        "peer",
        "list_topic_peers",
        &serde_json::json!({"topic": topic}),
    )?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("peers"))
        .and_then(|p| p.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

pub fn join_topic_peers(peer_token: &str, topic: &str, peers: &[String]) -> Result<()> {
    api::provider_call(
        peer_token,
        "peer",
        "gossip_join_peers",
        &serde_json::json!({
            "topic": topic,
            "peers": peers,
        }),
    )?;
    Ok(())
}

pub fn get_ticket(peer_token: &str) -> Result<Option<String>> {
    let response = api::provider_call(peer_token, "peer", "get_ticket", &serde_json::json!({}))?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("ticket"))
        .and_then(|t| t.as_str())
        .map(ToOwned::to_owned))
}

pub fn list_peers(peer_token: &str) -> Result<Vec<String>> {
    let response = api::provider_call(peer_token, "peer", "list_peers", &serde_json::json!({}))?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("peers"))
        .and_then(|p| p.as_array())
        .map(|peers| {
            peers
                .iter()
                .filter_map(|peer| peer.as_str().map(ToOwned::to_owned))
                .collect()
        })
        .unwrap_or_default())
}


pub fn send_gossip(
    peer_token: &str,
    topic: &str,
    sender_nick: &str,
    sender_id: &str,
    sender_session_id: Option<&str>,
    ts: u64,
    content: &str,
    signature: Option<&str>,
) -> Result<()> {
    let mut body = serde_json::json!({
        "topic": topic,
        "message": content,
        "sender": sender_nick,
    });
    if !sender_id.is_empty() {
        body["sender_id"] = serde_json::Value::String(sender_id.to_string());
    }
    if let Some(sender_session_id) = sender_session_id.filter(|value| !value.is_empty()) {
        body["sender_session_id"] = serde_json::Value::String(sender_session_id.to_string());
    }
    if ts > 0 {
        body["ts"] = serde_json::Value::from(ts);
    }
    if let Some(signature) = signature.filter(|value| !value.is_empty()) {
        body["signature"] = serde_json::Value::String(signature.to_string());
    }
    api::provider_call(peer_token, "peer", "gossip_send", &body)?;
    Ok(())
}

pub fn announce_presence(
    identity_token: &str,
    peer_token: &str,
    room: &str,
    sender_nick: &str,
    sender_id: &str,
    sender_session_id: Option<&str>,
    ticket: &str,
) -> Result<()> {
    if ticket.trim().is_empty() {
        return Ok(());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let payload = ChatPresenceAnnouncement {
        kind: "chat_presence_v1".to_string(),
        room: room.to_string(),
        did: sender_id.to_string(),
        session_id: sender_session_id
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned),
        nick: sender_nick.to_string(),
        ticket: ticket.to_string(),
        ts,
    };
    let content = serde_json::to_string(&payload)?;
    // Sign presence announcements — prevents peer impersonation via fake tickets
    let signature = sign_message(identity_token, sender_id, ts, &content).ok().flatten();
    send_gossip(
        peer_token,
        &chat_discovery_topic(room),
        sender_nick,
        sender_id,
        sender_session_id,
        ts,
        &content,
        signature.as_deref(),
    )
}

pub fn recv_presence_announcements(
    identity_token: &str,
    peer_token: &str,
    room: &str,
    consumer_id: &str,
    skip_sender_id: Option<&str>,
) -> Result<Vec<ChatPresenceAnnouncement>> {
    let messages = recv_messages(
        peer_token,
        &chat_discovery_topic(room),
        20,
        consumer_id,
        skip_sender_id,
    )?;
    Ok(messages
        .into_iter()
        .filter_map(|msg| {
            let presence = serde_json::from_str::<ChatPresenceAnnouncement>(&msg.content).ok()?;
            if presence.kind != "chat_presence_v1"
                || presence.room != room
                || presence.ticket.trim().is_empty()
            {
                return None;
            }
            // Verify presence signature — reject unsigned/forged presence to prevent
            // peer impersonation via fake tickets
            let verified = verify_message(identity_token, &msg).unwrap_or(false);
            if !verified {
                eprintln!(
                    "Dropping unverified presence from {} ({})",
                    presence.nick,
                    presence.did
                );
                return None;
            }
            Some(presence)
        })
        .collect())
}

pub fn attach_room_peer_until_joined(
    peer_token: &str,
    topic: &str,
    peer_ids: &[String],
) -> Result<bool> {
    if peer_ids.is_empty() {
        return Ok(false);
    }
    for _ in 0..20 {
        let _ = join_topic_peers(peer_token, topic, peer_ids);
        let joined = list_topic_peers(peer_token, topic)?;
        if peer_ids
            .iter()
            .any(|peer_id| joined.iter().any(|joined_peer| joined_peer == peer_id))
        {
            return Ok(true);
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
    Ok(false)
}

pub fn recv_messages(
    peer_token: &str,
    topic: &str,
    limit: usize,
    consumer_id: &str,
    skip_sender_id: Option<&str>,
) -> Result<Vec<Message>> {
    let mut payload = serde_json::json!({
        "topic": topic,
        "limit": limit,
        "consumer_id": consumer_id,
    });
    if let Some(skip_sender_id) = skip_sender_id.filter(|value| !value.is_empty()) {
        payload["skip_sender_id"] = serde_json::Value::String(skip_sender_id.to_string());
    }
    let response = api::provider_call(peer_token, "peer", "gossip_recv", &payload)?;
    Ok(response
        .get("data")
        .and_then(|d| d.get("messages"))
        .and_then(|m| m.as_array())
        .map(|messages| {
            messages
                .iter()
                .filter_map(|message| serde_json::from_value(message.clone()).ok())
                .collect()
        })
        .unwrap_or_default())
}
