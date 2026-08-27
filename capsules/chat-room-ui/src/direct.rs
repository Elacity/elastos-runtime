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
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct DirectConversationView {
    pub(super) conversation_id: String,
    pub(super) display_name: String,
    /// The relationship ended. History stays readable; composing stops.
    #[serde(default)]
    pub(super) removed: bool,
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
            Self::Expired => "Not delivered",
        }
    }
}

#[derive(Debug, Serialize)]
pub(super) struct DirectSendInput<'a> {
    pub(super) request_id: &'a str,
    pub(super) conversation_id: &'a str,
    pub(super) text: &'a str,
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

        // The terminal state parses and renders honestly: an abandoned
        // message says "Not delivered", never "Sending".
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
        assert_eq!(state.messages[0].delivery_state.label(), "Not delivered");
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
