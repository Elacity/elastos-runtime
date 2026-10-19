use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub(super) const DIRECT_API_BASE: &str = "/api/apps/chat-room/direct";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct DirectUiState {
    pub(super) selected_conversation_id: Option<String>,
    pub(super) conversations: Vec<DirectConversationView>,
    pub(super) messages: Vec<DirectMessageView>,
    pub(super) pending_send: Option<PendingDirectSend>,
    pub(super) notice: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectConversationList {
    pub(super) conversations: Vec<DirectConversationView>,
    #[serde(default)]
    pub(super) community_unread: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectConversationView {
    pub(super) conversation_id: String,
    pub(super) display_name: String,
    /// The relationship ended. History stays readable; composing stops.
    #[serde(default)]
    pub(super) removed: bool,
    /// A message arrived that this person has not opened yet.
    #[serde(default)]
    pub(super) unread: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectMessageList {
    pub(super) conversation_id: String,
    pub(super) messages: Vec<DirectMessageView>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectMessageView {
    pub(super) message_id: String,
    #[serde(default)]
    pub(super) request_id: Option<String>,
    pub(super) direction: DirectMessageDirection,
    pub(super) text: String,
    pub(super) created_at: u64,
    pub(super) delivery_state: DirectDeliveryState,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DirectMessageDirection {
    Incoming,
    Outgoing,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DirectDeliveryState {
    Received,
    Pending,
    ReceiptSettled,
    /// Terminal: the envelope's TTL passed with no receipt and the Runtime has
    /// stopped retrying. "Sending" would be a lie here.
    Expired,
}

impl DirectDeliveryState {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Received => "Received",
            Self::Pending => "Sending",
            Self::ReceiptSettled => "Sent",
            Self::Expired => "Expired",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct DirectSendInput<'a> {
    pub(super) request_id: &'a str,
    pub(super) conversation_id: &'a str,
    pub(super) text: &'a str,
    pub(super) retry_existing: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct DirectSendFailure {
    #[serde(skip)]
    pub(super) status: u16,
    #[serde(default)]
    pub(super) code: Option<String>,
}

pub(super) fn terminal_retry_detail(code: Option<&str>) -> Option<&'static str> {
    match code {
        Some("retry_expired") => Some("The 24-hour delivery window ended. Delivery is unconfirmed. Send again creates a new message."),
        Some("retry_unavailable") => Some("The original message is unavailable to retry. Delivery is unconfirmed. Send again creates a new message."),
        _ => None,
    }
}

/// How long an outgoing direct message stays "Sending" before Chat explains
/// it. The Runtime retries pending envelopes every 15 seconds, up to four per
/// pass (`DIRECT_SYNC_CADENCE_SECS`, `retry_pending`), so after a minute the
/// first automatic retries have already failed.
pub(super) const PENDING_NOTE_AFTER_MS: f64 = 60_000.0;

/// Remembers when this Chat session first saw each outgoing message pending,
/// on the browser's monotonic clock; the Runtime's `created_at` comes from
/// another clock and is not compared with it. A message that is no longer
/// pending is forgotten. Other conversations' entries are kept.
pub(super) fn track_pending_since(
    seen: &mut BTreeMap<String, f64>,
    messages: &[DirectMessageView],
    now_ms: f64,
) {
    for message in messages {
        if message.direction == DirectMessageDirection::Outgoing
            && message.delivery_state == DirectDeliveryState::Pending
        {
            seen.entry(message.message_id.clone()).or_insert(now_ms);
        } else {
            seen.remove(&message.message_id);
        }
    }
}

/// Milliseconds until a message first seen pending at `seen_at_ms` gets its
/// note; zero once it is due.
pub(super) fn pending_note_wait_ms(seen_at_ms: f64, now_ms: f64) -> f64 {
    (seen_at_ms + PENDING_NOTE_AFTER_MS - now_ms).max(0.0)
}

/// The note under a pending outgoing direct message: once it is due, or after
/// a manual Retry (with when it ran). The Runtime retries only messages to a
/// current contact (`retry_pending`), so a removed conversation gets no retry
/// claim. Chat cannot know whether the peer stored the message and only the
/// receipt is missing, or why a peer is unreachable.
pub(super) fn pending_delivery_note(
    due: bool,
    contact_current: bool,
    retried_at: Option<&str>,
) -> Option<String> {
    if !due && retried_at.is_none() {
        return None;
    }
    let mut note = String::from("Delivery unconfirmed.");
    if contact_current {
        note.push_str(" Your Home keeps trying while it is on, for up to 24 hours.");
    }
    if let Some(time) = retried_at {
        note.push_str(&format!(" Tried again at {time}."));
    }
    Some(note)
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum DirectSendStatus {
    Pending,
    ReceiptSettled,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectSendResponse {
    pub(super) status: DirectSendStatus,
}

pub(super) fn valid_direct_send_response(http_status: u16, status: DirectSendStatus) -> bool {
    matches!(
        (http_status, status),
        (200, DirectSendStatus::ReceiptSettled) | (202, DirectSendStatus::Pending)
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingDirectSend {
    pub(super) request_id: String,
    pub(super) conversation_id: String,
    pub(super) text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestedConversationDecision {
    Available,
    Unavailable,
    TemporarilyUnavailable,
}

pub(super) fn selected_conversation<'a>(
    conversations: &'a [DirectConversationView],
    conversation_id: &str,
) -> Option<&'a DirectConversationView> {
    conversations
        .iter()
        .find(|conversation| conversation.conversation_id == conversation_id)
}

pub(super) fn requested_conversation_decision(
    load_succeeded: bool,
    conversations: &[DirectConversationView],
    conversation_id: &str,
) -> RequestedConversationDecision {
    if !load_succeeded {
        return RequestedConversationDecision::TemporarilyUnavailable;
    }
    if selected_conversation(conversations, conversation_id).is_some() {
        RequestedConversationDecision::Available
    } else {
        RequestedConversationDecision::Unavailable
    }
}

pub(super) fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push('%');
            encoded.push(char::from(HEX[(byte >> 4) as usize]));
            encoded.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    encoded
}

pub(super) fn pending_direct_request_id<F>(
    pending: &mut Option<PendingDirectSend>,
    conversation_id: &str,
    text: &str,
    generate: F,
) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String>,
{
    if let Some(existing) = pending.as_ref() {
        if existing.conversation_id == conversation_id && existing.text == text {
            return Ok(existing.request_id.clone());
        }
    }
    let request_id = generate()?;
    *pending = Some(PendingDirectSend {
        request_id: request_id.clone(),
        conversation_id: conversation_id.to_string(),
        text: text.to_string(),
    });
    Ok(request_id)
}

pub(super) fn apply_direct_messages(
    state: &mut DirectUiState,
    response: DirectMessageList,
) -> Result<bool, ()> {
    if state.selected_conversation_id.as_deref() != Some(response.conversation_id.as_str()) {
        return Err(());
    }
    if state.messages == response.messages {
        return Ok(false);
    }
    state.messages = response.messages;
    Ok(true)
}

#[cfg(test)]
pub(super) fn selection_scoped_result<T>(
    requested_conversation_id: &str,
    current_conversation_id: Option<&str>,
    result: Result<T, u16>,
) -> Result<Option<T>, u16> {
    if current_conversation_id != Some(requested_conversation_id) {
        return Ok(None);
    }
    result.map(Some)
}

pub(super) fn should_clear_polled_transient_error(
    captured: Option<&str>,
    current: Option<&str>,
    current_is_transient: bool,
) -> bool {
    current_is_transient && captured.is_some() && captured == current
}

pub(super) fn remove_unavailable_conversation(state: &mut DirectUiState, conversation_id: &str) {
    state
        .conversations
        .retain(|conversation| conversation.conversation_id != conversation_id);
    if state.selected_conversation_id.as_deref() == Some(conversation_id) {
        state.selected_conversation_id = None;
        state.messages.clear();
        state.pending_send = None;
    }
    state.notice =
        Some("That conversation is no longer available. Choose another conversation.".to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation(id: &str, name: &str) -> DirectConversationView {
        DirectConversationView {
            conversation_id: id.to_string(),
            display_name: name.to_string(),
            removed: false,
            unread: false,
        }
    }

    #[test]
    fn selected_contact_requires_an_exact_runtime_conversation() {
        let conversations = vec![conversation("direct:one", "Alice")];
        assert_eq!(
            selected_conversation(&conversations, "direct:one")
                .map(|conversation| conversation.display_name.as_str()),
            Some("Alice")
        );
        assert!(selected_conversation(&conversations, "direct:other").is_none());
        assert!(selected_conversation(&conversations, "").is_none());
    }

    #[test]
    fn requested_conversation_distinguishes_missing_from_transient_list_failure() {
        let conversations = vec![conversation("direct:one", "Alice")];
        assert_eq!(
            requested_conversation_decision(true, &conversations, "direct:one"),
            RequestedConversationDecision::Available
        );
        assert_eq!(
            requested_conversation_decision(true, &conversations, "direct:missing"),
            RequestedConversationDecision::Unavailable
        );
        assert_eq!(
            requested_conversation_decision(false, &conversations, "direct:one"),
            RequestedConversationDecision::TemporarilyUnavailable
        );
    }

    #[test]
    fn opaque_conversation_selector_is_one_encoded_path_segment() {
        assert_eq!(
            encode_path_segment("direct:sha256:one/two"),
            "direct%3Asha256%3Aone%2Ftwo"
        );
    }

    #[test]
    fn failed_direct_send_reuses_one_request_id_until_intent_changes() {
        let mut pending = None;
        let first = pending_direct_request_id(&mut pending, "direct:one", "hello", || {
            Ok("chat-message:00000000000000000000000000000001".to_string())
        })
        .unwrap();
        assert_eq!(
            pending_direct_request_id(&mut pending, "direct:one", "hello", || {
                panic!("exact retry must not mint another request id")
            })
            .unwrap(),
            first
        );
        let changed = pending_direct_request_id(&mut pending, "direct:one", "changed", || {
            Ok("chat-message:00000000000000000000000000000002".to_string())
        })
        .unwrap();
        assert_ne!(first, changed);
    }

    #[test]
    fn pending_and_settled_responses_are_strict_delivery_states() {
        let pending: DirectSendResponse =
            serde_json::from_value(serde_json::json!({"status":"pending"})).unwrap();
        let settled: DirectSendResponse =
            serde_json::from_value(serde_json::json!({"status":"receipt_settled"})).unwrap();
        assert_eq!(pending.status, DirectSendStatus::Pending);
        assert_eq!(settled.status, DirectSendStatus::ReceiptSettled);
        assert!(valid_direct_send_response(202, pending.status));
        assert!(valid_direct_send_response(200, settled.status));
        assert!(!valid_direct_send_response(200, pending.status));
        assert!(!valid_direct_send_response(202, settled.status));
        assert!(serde_json::from_value::<DirectSendResponse>(
            serde_json::json!({"status":"accepted"})
        )
        .is_err());
    }

    #[test]
    fn direct_read_model_renders_only_bounded_product_fields() {
        let messages: DirectMessageList = serde_json::from_value(serde_json::json!({
            "conversation_id": "direct:one",
            "messages": [{
                "message_id": "message:one",
                "direction": "outgoing",
                "text": "hello",
                "created_at": 42,
                "delivery_state": "receipt_settled"
            }]
        }))
        .unwrap();
        let mut state = DirectUiState {
            selected_conversation_id: Some("direct:one".to_string()),
            ..DirectUiState::default()
        };
        assert!(apply_direct_messages(&mut state, messages).unwrap());
        assert_eq!(
            state.messages[0].direction,
            DirectMessageDirection::Outgoing
        );
        assert_eq!(state.messages[0].delivery_state.label(), "Sent");

        // Without a receipt, expiry leaves delivery unconfirmed.
        let expired: DirectMessageList = serde_json::from_value(serde_json::json!({
            "conversation_id": "direct:one",
            "messages": [{
                "message_id": "message:two",
                "direction": "outgoing",
                "text": "abandoned",
                "created_at": 43,
                "delivery_state": "expired"
            }]
        }))
        .unwrap();
        assert!(apply_direct_messages(&mut state, expired).unwrap());
        assert_eq!(
            state.messages[0].delivery_state,
            DirectDeliveryState::Expired
        );
        assert_eq!(state.messages[0].delivery_state.label(), "Expired");
    }

    #[test]
    fn pending_note_waits_on_this_session_clock_not_created_at() {
        let message = |id: &str, created_at: u64, state: &str| -> DirectMessageView {
            serde_json::from_value(serde_json::json!({
                "message_id": id, "request_id": format!("chat-message:{id}"),
                "direction": "outgoing", "text": "hi", "created_at": created_at,
                "delivery_state": state,
            }))
            .unwrap()
        };
        // One created_at far behind and one far ahead of the browser clock:
        // both wait exactly the threshold from when this session saw them.
        let mut seen = BTreeMap::new();
        let first = 5_000.0;
        let messages = [
            message("old", 1, "pending"),
            message("future", u64::MAX, "pending"),
        ];
        track_pending_since(&mut seen, &messages, first);
        track_pending_since(&mut seen, &messages, first + 30_000.0);
        for id in ["old", "future"] {
            assert_eq!(seen[id], first, "first sight is kept");
            assert_eq!(pending_note_wait_ms(seen[id], first), PENDING_NOTE_AFTER_MS);
            assert_eq!(
                pending_note_wait_ms(seen[id], first + PENDING_NOTE_AFTER_MS - 1.0),
                1.0
            );
            assert_eq!(
                pending_note_wait_ms(seen[id], first + PENDING_NOTE_AFTER_MS),
                0.0
            );
        }
        // Settled messages are forgotten; other conversations' are kept.
        seen.insert("elsewhere".to_string(), 1.0);
        track_pending_since(&mut seen, &[message("old", 1, "receipt_settled")], first);
        assert!(!seen.contains_key("old") && seen.contains_key("elsewhere"));
        let incoming: DirectMessageView = serde_json::from_value(serde_json::json!({
            "message_id": "in", "direction": "incoming", "text": "hi", "created_at": 1,
            "delivery_state": "received",
        }))
        .unwrap();
        track_pending_since(&mut seen, &[incoming], first);
        assert!(!seen.contains_key("in"));
    }

    #[test]
    fn pending_note_claims_retries_only_for_a_current_contact() {
        assert_eq!(pending_delivery_note(false, true, None), None);
        assert_eq!(
            pending_delivery_note(true, true, None).unwrap(),
            "Delivery unconfirmed. Your Home keeps trying while it is on, for up to 24 hours."
        );
        // The Runtime does not retry a removed contact's messages.
        assert_eq!(
            pending_delivery_note(true, false, None).unwrap(),
            "Delivery unconfirmed."
        );
        // A manual Retry shows when it ran, even before the threshold.
        assert_eq!(
            pending_delivery_note(false, true, Some("14:03")).unwrap(),
            "Delivery unconfirmed. Your Home keeps trying while it is on, for up to 24 hours. Tried again at 14:03."
        );
    }

    #[test]
    fn retry_fields_are_opted_product_fields_and_terminal_codes_are_distinct() {
        let messages: DirectMessageList = serde_json::from_value(serde_json::json!({
            "conversation_id":"direct:one", "messages":[{
                "message_id":"message:one", "request_id":"chat-message:one",
                "direction":"outgoing", "text":"hello", "created_at":42, "delivery_state":"pending"
            }]
        }))
        .unwrap();
        assert_eq!(
            messages.messages[0].request_id.as_deref(),
            Some("chat-message:one")
        );
        assert_eq!(messages.messages[0].delivery_state.label(), "Sending");
        let legacy: DirectConversationList =
            serde_json::from_value(serde_json::json!({"conversations":[]})).unwrap();
        assert_eq!(legacy.community_unread, None);
        let current: DirectConversationList =
            serde_json::from_value(serde_json::json!({"conversations":[],"community_unread":true}))
                .unwrap();
        assert_eq!(current.community_unread, Some(true));
        assert!(terminal_retry_detail(Some("retry_expired"))
            .unwrap()
            .contains("24-hour"));
        assert!(terminal_retry_detail(Some("retry_unavailable"))
            .unwrap()
            .contains("unavailable"));
        assert!(terminal_retry_detail(Some("conflict")).is_none());
        assert!(terminal_retry_detail(None).is_none());
        let retry = DirectSendInput {
            request_id: "old",
            conversation_id: "direct:one",
            text: "hello",
            retry_existing: true,
        };
        assert_eq!(serde_json::to_value(retry).unwrap()["retry_existing"], true);
    }

    #[test]
    fn stale_direct_read_success_and_error_cannot_change_a_new_selection() {
        assert_eq!(
            selection_scoped_result("direct:old", Some("direct:new"), Ok::<_, u16>(42)),
            Ok(None)
        );
        let shared_state = DirectUiState::default();
        let stale_403 = selection_scoped_result::<()>(
            "direct:old",
            shared_state.selected_conversation_id.as_deref(),
            Err(403),
        );
        assert_eq!(stale_403, Ok(None));
        assert!(shared_state.selected_conversation_id.is_none());
        assert!(shared_state.notice.is_none());
        assert_eq!(
            selection_scoped_result::<()>("direct:current", Some("direct:current"), Err(403)),
            Err(403)
        );
    }

    #[test]
    fn poll_clears_only_the_same_transient_error_it_observed() {
        assert!(should_clear_polled_transient_error(
            Some("old error"),
            Some("old error"),
            true,
        ));
        assert!(!should_clear_polled_transient_error(
            Some("old error"),
            Some("new error"),
            true,
        ));
        assert!(!should_clear_polled_transient_error(
            Some("old error"),
            Some("old error"),
            false,
        ));
    }

    #[test]
    fn unavailable_current_conversation_returns_to_selector_without_a_fallback() {
        let mut state = DirectUiState {
            selected_conversation_id: Some("direct:old".to_string()),
            conversations: vec![
                conversation("direct:old", "Old"),
                conversation("direct:other", "Other"),
            ],
            pending_send: Some(PendingDirectSend {
                request_id: "chat-message:00000000000000000000000000000001".to_string(),
                conversation_id: "direct:old".to_string(),
                text: "hello".to_string(),
            }),
            ..DirectUiState::default()
        };
        remove_unavailable_conversation(&mut state, "direct:old");
        assert!(state.selected_conversation_id.is_none());
        assert!(state.pending_send.is_none());
        assert_eq!(
            state.conversations,
            vec![conversation("direct:other", "Other")]
        );
        assert_eq!(
            state.notice.as_deref(),
            Some("That conversation is no longer available. Choose another conversation.")
        );
    }

    #[test]
    fn message_projection_rejects_authority_and_topology_fields() {
        assert!(
            serde_json::from_value::<DirectMessageList>(serde_json::json!({
                "conversation_id": "direct:one",
                "messages": [{
                    "message_id": "message:one",
                    "direction": "incoming",
                    "text": "hello",
                    "created_at": 42,
                    "delivery_state": "received",
                    "sender_profile_did": "did:key:hidden"
                }]
            }))
            .is_err()
        );
    }
}
