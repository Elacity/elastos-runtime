use base64::Engine as _;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use js_sys::{Array, Function, Object as JsObject, Promise, Reflect, Uint8Array};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    window, Blob, Document, Element, Event, HtmlButtonElement, HtmlElement, HtmlFormElement,
    HtmlInputElement, HtmlTextAreaElement, MessageEvent, Node, RequestCredentials, Storage,
};

mod direct;

use direct::{
    apply_direct_messages, encode_path_segment, pending_direct_request_id,
    remove_unavailable_conversation, requested_conversation_decision, selected_conversation,
    should_clear_polled_transient_error, valid_direct_send_response, DirectConversationList,
    DirectMessageDirection, DirectMessageList, DirectSendInput, DirectSendResponse, DirectUiState,
    RequestedConversationDecision, DIRECT_API_BASE,
};

const BROWSER_SESSION_API_BASE: &str = "/api/browser/session";
const BROWSER_SESSION_REQUEST_STORAGE_KEY: &str = "elastos.browser_session.request_id";
const CHAT_ROOM_API_BASE: &str = "/api/apps/chat-room";
const ROOM_ACCESS_CAPABILITIES: [&str; 1] = ["room.access"];
const SHELL_ROOM_POLL_INTERVAL_MS: u32 = 1_000;
const GATEWAY_POLL_INTERVAL_MS: u32 = 3_000;
const AUTO_SCROLL_THRESHOLD_PX: i32 = 48;
const HOME_TOKEN_HEADER: &str = "x-elastos-home-token";
const DISPLAY_NAME_REQUIRED_ERROR: &str = "Enter your name.";
const APPROVAL_REQUESTED_BADGE: &str = "Waiting";
const APPROVAL_REQUESTED_DETAIL: &str = "Waiting for approval.";
const SHELL_ACCESS_UNAVAILABLE_DETAIL: &str =
    "This device is not part of this conversation yet. Join from an invite or connect it first.";
const EMOJI_BUTTON_IDS: [(&str, &str); 12] = [
    ("emoji-wave", "👋"),
    ("emoji-thumbsup", "👍"),
    ("emoji-heart", "❤️"),
    ("emoji-joy", "😂"),
    ("emoji-fire", "🔥"),
    ("emoji-party", "🎉"),
    ("emoji-clap", "👏"),
    ("emoji-eyes", "👀"),
    ("emoji-pray", "🙏"),
    ("emoji-sad", "😢"),
    ("emoji-think", "🤔"),
    ("emoji-elephant", "🐘"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccessMode {
    Gateway,
    Shell,
}

#[derive(Debug, Clone)]
struct AppConfig {
    access_mode: AccessMode,
    home_token: Option<String>,
    initial_join_invite: Option<String>,
    initial_direct_conversation_id: Option<String>,
    browser_session_request_storage_key: String,
}

#[derive(Debug, Clone, Default)]
struct AppState {
    request_id: Option<String>,
    pending_chat_send: Option<PendingChatSend>,
    selection_generation: u64,
    room_mode_known: bool,
    collaboration_configured: bool,
    session_active: bool,
    close_leave_sent: bool,
    poll_loop_started: bool,
    show_participants: bool,
    show_access_controls: bool,
    force_message_follow: bool,
    display_name: String,
    status_badge: String,
    status_detail: String,
    error_text: Option<String>,
    error_transient: bool,
    browser_access_allowed: bool,
    browser_access_block_reason: Option<String>,
    latest_seq: u64,
    objects: Vec<ConversationObjectView>,
    participants: Vec<ParticipantView>,
    pending_requests: Vec<PendingRequestView>,
    active_sessions: Vec<ActiveSessionView>,
    room_control: SummaryRoomControlView,
    attachment_urls: BTreeMap<String, String>,
    join_invite_url: Option<String>,
    direct: DirectUiState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingChatSend {
    request_id: String,
    body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectionGuard {
    generation: u64,
    selected_conversation_id: Option<String>,
}

struct App {
    config: AppConfig,
    state: RefCell<AppState>,
    document: Document,
    body: HtmlElement,
    session_storage: Option<Storage>,
    status_badge: Option<HtmlElement>,
    status_detail: Option<HtmlElement>,
    error_text: HtmlElement,
    gateway_ui: Option<GatewayUi>,
    chat_card: HtmlElement,
    conversation_selector: HtmlElement,
    conversation_avatar: HtmlElement,
    conversation_title: HtmlElement,
    conversation_detail: HtmlElement,
    presence_card: HtmlElement,
    message_list: HtmlElement,
    composer_form: HtmlFormElement,
    message_input: HtmlTextAreaElement,
    attach_button: HtmlButtonElement,
    send_button: HtmlButtonElement,
    participant_toggle: HtmlButtonElement,
    room_access_toggle: HtmlButtonElement,
    participant_close: HtmlButtonElement,
    participant_scrim: HtmlElement,
    emoji_buttons: Vec<HtmlButtonElement>,
    participant_count: HtmlElement,
    participant_list: HtmlElement,
    browser_access_section: HtmlElement,
    browser_access_count: HtmlElement,
    browser_access_list: HtmlElement,
    room_access_section: HtmlElement,
    room_policy_list: HtmlElement,
    conversation_join_section: HtmlElement,
    conversation_join_form: HtmlFormElement,
    conversation_join_input: HtmlInputElement,
    conversation_join_submit: HtmlButtonElement,
    conversation_invite_create: HtmlButtonElement,
    conversation_invite_output_row: HtmlElement,
    conversation_invite_output: HtmlInputElement,
    conversation_invite_copy: HtmlButtonElement,
    node_list: HtmlElement,
}

struct GatewayUi {
    browser_access_stage: HtmlElement,
    browser_access_status_row: HtmlElement,
    browser_access_form: HtmlFormElement,
    display_name_input: HtmlInputElement,
    browser_access_submit: HtmlButtonElement,
    reset_button: HtmlButtonElement,
}

#[derive(Debug, Clone, Deserialize)]
struct BrowserSessionRequestOutput {
    request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct BrowserSessionStatusOutput {
    status: String,
    expires_at: Option<u64>,
    denial_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct ShellSessionStartOutput {
    poll: RoomPollView,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellSessionBootstrapFailure {
    Unauthorized,
    Unavailable,
}

impl ShellSessionBootstrapFailure {
    fn from_request_error(error: &str) -> Self {
        if is_session_error(error) {
            Self::Unauthorized
        } else {
            Self::Unavailable
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::Unauthorized => {
                "Chat session bootstrap was not authorized. Reopen Chat from Home."
            }
            Self::Unavailable => "Chat session bootstrap failed. Reopen Chat from Home.",
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ConversationObjectView {
    seq: u64,
    sender: String,
    /// `Some(true)`: `sender` is the sender's verified Profile display name.
    /// `Some(false)`/`None`: configured shared Chat omits the row; other modes
    /// keep their existing server-stamped naming.
    #[serde(default)]
    sender_profile_verified: Option<bool>,
    #[serde(default)]
    from_current_session: bool,
    kind: ConversationObjectKind,
    body: Option<String>,
    emoji: Option<String>,
    link: Option<LinkPreviewView>,
    attachment: Option<AttachmentView>,
    created_at: u64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ConversationObjectKind {
    System,
    Text,
    Emoji,
    Link,
    Attachment,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct LinkPreviewView {
    url: String,
    host: String,
    title: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct AttachmentView {
    attachment_id: String,
    file_name: String,
    mime_type: String,
    size_bytes: u64,
    is_image: bool,
    is_audio: bool,
    is_video: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ParticipantView {
    display_name: String,
    /// Same contract as `ConversationObjectView::sender_profile_verified`.
    #[serde(default)]
    profile_verified: Option<bool>,
    device_label: String,
    last_seen_at: u64,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    local_session_count: usize,
    #[serde(default)]
    is_current_session: bool,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct PendingRequestView {
    request_id: String,
    display_name: String,
    device_label: String,
    requested_at: u64,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ActiveSessionView {
    session_id: String,
    display_name: String,
    device_label: String,
    approved_at: u64,
    expires_at: u64,
    last_seen_at: u64,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    member_bound: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomPollView {
    #[allow(dead_code)]
    room_slug: String,
    display_name: String,
    latest_seq: u64,
    #[serde(default)]
    participants: Vec<ParticipantView>,
    #[serde(default)]
    objects: Vec<ConversationObjectView>,
    #[serde(default)]
    transport: RoomTransportView,
}

#[derive(Debug, Clone, Deserialize)]
struct SummaryView {
    #[allow(dead_code)]
    room_slug: String,
    #[allow(dead_code)]
    pending_count: usize,
    #[allow(dead_code)]
    active_session_count: usize,
    #[serde(default)]
    local_runtime_role: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    ephemeral_hosted_guest_url: Option<String>,
    #[serde(default)]
    room_control: SummaryRoomControlView,
    #[serde(default)]
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    pending_requests: Vec<PendingRequestView>,
    #[serde(default)]
    active_sessions: Vec<ActiveSessionView>,
    #[serde(default)]
    transport: RoomTransportView,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
struct RoomTransportView {
    #[serde(default)]
    configured: bool,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
struct SummaryRoomControlView {
    #[serde(default)]
    access_policy: RoomAccessPolicyView,
    #[serde(default)]
    members: Vec<RoomMemberView>,
    #[serde(default)]
    pending_invites: Vec<RoomInviteView>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RoomAccessPolicyView {
    #[serde(default = "room_access_policy_enabled_default")]
    allow_guest_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    allow_member_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    allow_members_to_host_guests: bool,
}

impl Default for RoomAccessPolicyView {
    fn default() -> Self {
        Self {
            allow_guest_invites: true,
            allow_member_invites: true,
            allow_members_to_host_guests: true,
        }
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RoomMemberView {
    role: String,
    added_at: u64,
    #[serde(default)]
    profile_card: Option<RoomProfileCardView>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RoomProfileCardView {
    display_name: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RoomInviteView {
    invite_id: String,
    role: String,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug, Serialize)]
struct BrowserSessionRequestInput<'a> {
    display_name: &'a str,
    device_label: &'a str,
    capabilities: &'a [&'a str],
}

#[derive(Debug, Serialize)]
struct RoomPollInput {
    since: u64,
}

#[derive(Debug, Serialize)]
struct SendMessageInput<'a> {
    request_id: &'a str,
    body: &'a str,
}

#[derive(Debug, Serialize)]
struct AttachmentUploadStartInput<'a> {
    file_name: &'a str,
    mime_type: &'a str,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct AttachmentUploadStartResponse {
    upload_id: String,
    chunk_size_bytes: u64,
}

#[derive(Debug, Serialize)]
struct RoomAccessPolicyInput {
    allow_guest_invites: bool,
    allow_member_invites: bool,
    allow_members_to_host_guests: bool,
}

#[derive(Debug, Serialize)]
struct RoomInviteRevokeInput<'a> {
    invite_id: &'a str,
}

#[derive(Debug, Serialize)]
struct ConversationJoinInviteCreateInput {}

#[derive(Debug, Deserialize)]
struct ConversationJoinInviteView {
    invite_url: String,
    #[allow(dead_code)]
    token: String,
    #[allow(dead_code)]
    room_title: String,
    #[allow(dead_code)]
    invited_by: String,
    #[allow(dead_code)]
    expires_at: u64,
}

#[derive(Debug, Serialize)]
struct ConversationJoinInviteJoinInput<'a> {
    invite: &'a str,
}

#[derive(Debug, Deserialize)]
struct ConversationJoinInviteJoinResponse {
    #[allow(dead_code)]
    status: String,
    room_title: String,
    #[allow(dead_code)]
    issuer_gateway: String,
    #[allow(dead_code)]
    invite_id: String,
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("document unavailable"))?;
    let config = load_config(&document)?;
    let session_storage = match config.access_mode {
        AccessMode::Gateway => Some(
            window
                .session_storage()?
                .ok_or_else(|| JsValue::from_str("sessionStorage unavailable"))?,
        ),
        AccessMode::Shell => None,
    };
    let state = load_state(session_storage.as_ref(), &config);
    let gateway_ui = if config.access_mode == AccessMode::Gateway {
        Some(GatewayUi {
            browser_access_stage: element_by_id(&document, "browser-access-stage")?,
            browser_access_status_row: element_by_id(&document, "browser-access-status-row")?,
            browser_access_form: form_by_id(&document, "browser-access-form")?,
            display_name_input: input_by_id(&document, "display-name")?,
            browser_access_submit: button_by_id(&document, "browser-access-submit")?,
            reset_button: button_by_id(&document, "reset-button")?,
        })
    } else {
        None
    };

    let app = Rc::new(App {
        config,
        state: RefCell::new(state),
        document: document.clone(),
        body: document
            .body()
            .ok_or_else(|| JsValue::from_str("document body unavailable"))?,
        session_storage,
        status_badge: optional_element_by_id(&document, "status-badge"),
        status_detail: optional_element_by_id(&document, "status-detail"),
        error_text: element_by_id(&document, "error-text")?,
        gateway_ui,
        chat_card: element_by_id(&document, "chat-card")?,
        conversation_selector: element_by_id(&document, "conversation-selector")?,
        conversation_avatar: element_by_id(&document, "conversation-avatar")?,
        conversation_title: element_by_id(&document, "conversation-title")?,
        conversation_detail: element_by_id(&document, "conversation-detail")?,
        presence_card: element_by_id(&document, "presence-card")?,
        message_list: element_by_id(&document, "message-list")?,
        composer_form: form_by_id(&document, "composer-form")?,
        message_input: textarea_by_id(&document, "message-input")?,
        attach_button: button_by_id(&document, "attach-button")?,
        send_button: button_by_id(&document, "send-button")?,
        participant_toggle: button_by_id(&document, "participant-toggle")?,
        room_access_toggle: button_by_id(&document, "room-access-toggle")?,
        participant_close: button_by_id(&document, "participant-close")?,
        participant_scrim: element_by_id(&document, "participant-scrim")?,
        emoji_buttons: EMOJI_BUTTON_IDS
            .iter()
            .map(|(id, _)| button_by_id(&document, id))
            .collect::<Result<Vec<_>, _>>()?,
        participant_count: element_by_id(&document, "participant-count")?,
        participant_list: element_by_id(&document, "participant-list")?,
        browser_access_section: element_by_id(&document, "browser-access-section")?,
        browser_access_count: element_by_id(&document, "browser-access-count")?,
        browser_access_list: element_by_id(&document, "browser-access-list")?,
        room_access_section: element_by_id(&document, "room-access-section")?,
        room_policy_list: element_by_id(&document, "room-policy-list")?,
        conversation_join_section: element_by_id(&document, "conversation-join-section")?,
        conversation_join_form: form_by_id(&document, "conversation-join-form")?,
        conversation_join_input: input_by_id(&document, "conversation-join-input")?,
        conversation_join_submit: button_by_id(&document, "conversation-join-submit")?,
        conversation_invite_create: button_by_id(&document, "conversation-invite-create")?,
        conversation_invite_output_row: element_by_id(&document, "conversation-invite-output-row")?,
        conversation_invite_output: input_by_id(&document, "conversation-invite-output")?,
        conversation_invite_copy: button_by_id(&document, "conversation-invite-copy")?,
        node_list: element_by_id(&document, "node-list")?,
    });

    if let Some(gateway_ui) = &app.gateway_ui {
        let state = app.state.borrow();
        gateway_ui.display_name_input.set_value(&state.display_name);
    }

    app.bind_events()?;
    app.render()?;
    app.hydrate_defaults();
    if !app.is_shell_mode() {
        app.restore_session();
        app.start_poll_loop();
    }

    Ok(())
}

impl App {
    fn is_shell_mode(&self) -> bool {
        self.config.access_mode == AccessMode::Shell
    }

    fn default_status_badge(&self) -> String {
        default_status_badge_for_mode(self.config.access_mode)
    }

    fn default_status_detail(&self) -> String {
        default_status_detail_for_mode(self.config.access_mode)
    }

    fn poll_interval_ms(&self) -> u32 {
        if self.is_shell_mode() {
            SHELL_ROOM_POLL_INTERVAL_MS
        } else {
            GATEWAY_POLL_INTERVAL_MS
        }
    }

    fn start_poll_loop(self: &Rc<Self>) {
        {
            let mut state = self.state.borrow_mut();
            if state.poll_loop_started {
                return;
            }
            state.poll_loop_started = true;
        }
        let poll_app = Rc::clone(self);
        spawn_local(async move {
            loop {
                poll_app.poll_and_render_once().await;
                TimeoutFuture::new(poll_app.poll_interval_ms()).await;
            }
        });
    }

    async fn poll_and_render_once(&self) {
        if self.is_direct_mode() {
            let (selection_guard, polled_transient_error) = {
                let state = self.state.borrow();
                (
                    current_selection_guard(&state),
                    state
                        .error_transient
                        .then(|| state.error_text.clone())
                        .flatten(),
                )
            };
            let result = self
                .refresh_direct_messages_for_guard(&selection_guard)
                .await;
            if !self.selection_guard_is_current(&selection_guard) {
                return;
            }
            match result {
                Ok(changed) => {
                    let cleared_transient_error = {
                        let mut state = self.state.borrow_mut();
                        if should_clear_polled_transient_error(
                            polled_transient_error.as_deref(),
                            state.error_text.as_deref(),
                            state.error_transient,
                        ) {
                            state.error_text = None;
                            state.error_transient = false;
                            true
                        } else {
                            false
                        }
                    };
                    if changed || cleared_transient_error {
                        let _ = self.render();
                    }
                }
                Err(403) => {
                    let stale_id = self.state.borrow().direct.selected_conversation_id.clone();
                    self.return_to_conversation_selector(stale_id.as_deref());
                    let _ = self.render();
                }
                Err(_) => {
                    self.set_transient_error(Some(
                        "Direct messages are temporarily unavailable.".to_string(),
                    ));
                    let _ = self.render();
                }
            }
            return;
        }
        let shared_selection_guard = self.is_shell_mode().then(|| {
            let state = self.state.borrow();
            current_selection_guard(&state)
        });
        let had_transient_error = {
            let state = self.state.borrow();
            state.error_text.is_some() && state.error_transient
        };
        match self
            .poll_once_for_guard(shared_selection_guard.as_ref())
            .await
        {
            Ok(changed) => {
                let mut shell_summary_changed = false;
                if self.is_shell_mode() {
                    match self
                        .refresh_shell_summary_for_guard(
                            shared_selection_guard
                                .as_ref()
                                .expect("shell mode must capture a selection guard"),
                        )
                        .await
                    {
                        Ok(changed) => shell_summary_changed = changed,
                        Err(err) => {
                            if shared_selection_guard
                                .as_ref()
                                .is_some_and(|guard| self.selection_guard_is_current(guard))
                            {
                                self.set_transient_error(Some(err));
                                shell_summary_changed = true;
                            }
                        }
                    }
                }
                if shared_selection_guard
                    .as_ref()
                    .is_some_and(|guard| !self.selection_guard_is_current(guard))
                {
                    return;
                }
                if had_transient_error {
                    self.clear_error();
                }
                if changed || had_transient_error || shell_summary_changed {
                    let _ = self.render();
                }
            }
            Err(err) => {
                self.set_transient_error(Some(err));
                let _ = self.render();
            }
        }
    }

    fn room_api_url(&self, suffix: &str) -> String {
        format!("{}{}", CHAT_ROOM_API_BASE, suffix)
    }

    fn is_direct_mode(&self) -> bool {
        self.state
            .borrow()
            .direct
            .selected_conversation_id
            .is_some()
    }

    fn selection_guard_is_current(&self, guard: &SelectionGuard) -> bool {
        selection_guard_matches(&self.state.borrow(), guard)
    }

    fn home_token_headers(&self) -> Vec<(&'static str, String)> {
        self.config
            .home_token
            .as_ref()
            .map(|token| vec![(HOME_TOKEN_HEADER, token.clone())])
            .unwrap_or_default()
    }

    fn room_request_headers(&self) -> Vec<(&'static str, String)> {
        if self.is_shell_mode() {
            self.home_token_headers()
        } else {
            Vec::new()
        }
    }

    async fn refresh_direct_conversations(&self) -> Result<bool, u16> {
        if !self.is_shell_mode() {
            return Ok(false);
        }
        let response: DirectConversationList = direct_get_json(
            &format!("{DIRECT_API_BASE}/conversations"),
            &self.home_token_headers(),
        )
        .await?;
        if response.conversations.iter().any(|conversation| {
            conversation.conversation_id.trim().is_empty()
                || conversation.display_name.trim().is_empty()
        }) {
            return Err(500);
        }
        let mut state = self.state.borrow_mut();
        if state.direct.conversations == response.conversations {
            return Ok(false);
        }
        state.direct.conversations = response.conversations;
        Ok(true)
    }

    fn return_to_conversation_selector(&self, unavailable_conversation_id: Option<&str>) {
        let mut state = self.state.borrow_mut();
        if let Some(unavailable_conversation_id) = unavailable_conversation_id {
            remove_unavailable_conversation(&mut state.direct, unavailable_conversation_id);
        } else {
            clear_selected_direct_conversation(&mut state);
            state.direct.notice = Some(
                "That conversation is no longer available. Choose another conversation."
                    .to_string(),
            );
        }
        state.error_text = state.direct.notice.clone();
        state.error_transient = false;
    }

    async fn refresh_direct_messages_for_guard(&self, guard: &SelectionGuard) -> Result<bool, u16> {
        let conversation_id = guard.selected_conversation_id.clone().ok_or(403u16)?;
        let conversations_result: Result<DirectConversationList, u16> = direct_get_json(
            &format!("{DIRECT_API_BASE}/conversations"),
            &self.home_token_headers(),
        )
        .await;
        if !self.selection_guard_is_current(guard) {
            return Ok(false);
        }
        let conversations = conversations_result?;
        if conversations.conversations.iter().any(|conversation| {
            conversation.conversation_id.trim().is_empty()
                || conversation.display_name.trim().is_empty()
        }) {
            return Err(500);
        }
        let selected_is_available =
            selected_conversation(&conversations.conversations, &conversation_id).is_some();
        if !selected_is_available {
            return Err(403);
        }
        let messages_result: Result<DirectMessageList, u16> = direct_get_json(
            &format!(
                "{DIRECT_API_BASE}/conversations/{}/messages",
                encode_path_segment(&conversation_id)
            ),
            &self.home_token_headers(),
        )
        .await;
        if !self.selection_guard_is_current(guard) {
            return Ok(false);
        }
        let response = messages_result?;
        if response
            .messages
            .iter()
            .any(|message| message.message_id.trim().is_empty() || message.text.trim().is_empty())
        {
            return Err(500);
        }
        let mut state = self.state.borrow_mut();
        let Some(result) = apply_direct_refresh_if_current(
            &mut state,
            guard,
            conversations.conversations,
            response,
        ) else {
            return Ok(false);
        };
        result
    }

    fn apply_summary(&self, summary: &SummaryView) -> bool {
        apply_summary_state(
            &mut self.state.borrow_mut(),
            summary,
            self.config.access_mode,
        )
    }

    fn bind_events(self: &Rc<Self>) -> Result<(), JsValue> {
        if let Some(gateway_ui) = &self.gateway_ui {
            let browser_access_app = Rc::clone(self);
            let browser_access_submit =
                Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
                    event.prevent_default();
                    let app = Rc::clone(&browser_access_app);
                    spawn_local(async move {
                        app.clear_error();
                        if let Err(err) = app.submit_browser_session_request().await {
                            app.set_error(Some(err));
                        }
                        let _ = app.render();
                    });
                }));
            gateway_ui
                .browser_access_form
                .add_event_listener_with_callback(
                    "submit",
                    browser_access_submit.as_ref().unchecked_ref(),
                )?;
            browser_access_submit.forget();

            let browser_access_validate_app = Rc::clone(self);
            let browser_access_click =
                Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                    let Some(gateway_ui) = browser_access_validate_app.gateway_ui.as_ref() else {
                        return;
                    };
                    if gateway_ui.display_name_input.value().trim().is_empty() {
                        browser_access_validate_app
                            .set_error(Some(DISPLAY_NAME_REQUIRED_ERROR.to_string()));
                    } else {
                        browser_access_validate_app.clear_error_if(DISPLAY_NAME_REQUIRED_ERROR);
                    }
                    let _ = browser_access_validate_app.render();
                }));
            gateway_ui
                .browser_access_submit
                .add_event_listener_with_callback(
                    "click",
                    browser_access_click.as_ref().unchecked_ref(),
                )?;
            browser_access_click.forget();

            let browser_access_input_app = Rc::clone(self);
            let browser_access_input =
                Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                    let Some(gateway_ui) = browser_access_input_app.gateway_ui.as_ref() else {
                        return;
                    };
                    if gateway_ui.display_name_input.value().trim().is_empty() {
                        return;
                    }
                    browser_access_input_app.clear_error_if(DISPLAY_NAME_REQUIRED_ERROR);
                    let _ = browser_access_input_app.render();
                }));
            gateway_ui
                .display_name_input
                .add_event_listener_with_callback(
                    "input",
                    browser_access_input.as_ref().unchecked_ref(),
                )?;
            browser_access_input.forget();

            let invalid_app = Rc::clone(self);
            let invalid_display_name =
                Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
                    event.prevent_default();
                    invalid_app.set_error(Some(DISPLAY_NAME_REQUIRED_ERROR.to_string()));
                    let _ = invalid_app.render();
                }));
            gateway_ui
                .display_name_input
                .add_event_listener_with_callback(
                    "invalid",
                    invalid_display_name.as_ref().unchecked_ref(),
                )?;
            invalid_display_name.forget();

            let reset_app = Rc::clone(self);
            let reset_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                reset_app.reset_browser_session_request();
                let _ = reset_app.render();
            }));
            gateway_ui
                .reset_button
                .add_event_listener_with_callback("click", reset_click.as_ref().unchecked_ref())?;
            reset_click.forget();
        }

        let send_app = Rc::clone(self);
        let send_submit = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            event.prevent_default();
            let app = Rc::clone(&send_app);
            spawn_local(async move {
                app.clear_error();
                if let Err(err) = app.send_message().await {
                    app.set_error(Some(err));
                }
                let _ = app.render();
            });
        }));
        self.composer_form
            .add_event_listener_with_callback("submit", send_submit.as_ref().unchecked_ref())?;
        send_submit.forget();

        let edit_app = Rc::clone(self);
        let message_edit = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            let body = edit_app.message_input.value().trim().to_string();
            let mut state = edit_app.state.borrow_mut();
            if let Some(conversation_id) = state.direct.selected_conversation_id.clone() {
                if state.direct.pending_send.as_ref().is_some_and(|pending| {
                    pending.conversation_id != conversation_id || pending.text != body
                }) {
                    state.direct.pending_send = None;
                }
                return;
            }
            if state
                .pending_chat_send
                .as_ref()
                .is_some_and(|pending| pending.body != body)
            {
                state.pending_chat_send = None;
            }
        }));
        self.message_input
            .add_event_listener_with_callback("input", message_edit.as_ref().unchecked_ref())?;
        message_edit.forget();

        let selector_app = Rc::clone(self);
        let selector_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            let Some(target) = event.target() else {
                return;
            };
            let Some(button) = target
                .dyn_ref::<Element>()
                .cloned()
                .or_else(|| {
                    target
                        .dyn_ref::<Node>()
                        .and_then(|node| node.parent_element())
                })
                .and_then(|element| element.closest("[data-conversation-choice]").ok().flatten())
            else {
                return;
            };
            let choice = resolve_conversation_choice(
                button.get_attribute("data-conversation-choice").as_deref(),
                target
                    .dyn_ref::<Node>()
                    .and_then(|node| node.parent_element())
                    .and_then(|element| {
                        element.closest("[data-conversation-choice]").ok().flatten()
                    })
                    .and_then(|element| element.get_attribute("data-conversation-choice"))
                    .as_deref(),
            );
            let Some(choice) = choice else {
                return;
            };
            let selection_guard = {
                let mut state = selector_app.state.borrow_mut();
                state.error_text = None;
                state.error_transient = false;
                if choice == "shared" {
                    commit_shared_selection(&mut state)
                } else {
                    let Some(guard) = commit_direct_selection(&mut state, &choice) else {
                        return;
                    };
                    guard
                }
            };
            let _ = selector_app.render();
            selector_app.start_poll_loop();
            let app = Rc::clone(&selector_app);
            spawn_local(async move {
                if choice == "shared" {
                    if let Err(error) = app.ensure_shell_session_for_guard(&selection_guard).await {
                        if app.selection_guard_is_current(&selection_guard) {
                            app.set_error(Some(error));
                        }
                    }
                } else if let Err(status) = app
                    .refresh_direct_messages_for_guard(&selection_guard)
                    .await
                {
                    if app.selection_guard_is_current(&selection_guard) {
                        if status == 403 {
                            app.return_to_conversation_selector(Some(&choice));
                        } else {
                            app.set_error(Some(
                                "Direct messages are temporarily unavailable.".to_string(),
                            ));
                        }
                    }
                }
                if app.selection_guard_is_current(&selection_guard) {
                    let _ = app.render();
                }
            });
        }));
        self.conversation_selector
            .add_event_listener_with_callback("click", selector_click.as_ref().unchecked_ref())?;
        selector_click.forget();

        let attach_trigger_app = Rc::clone(self);
        let attach_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            event.prevent_default();
            attach_trigger_app.clear_error();
            match attach_trigger_app.open_library_from_home() {
                Ok(true) => {}
                Ok(false) => {
                    attach_trigger_app
                        .set_error(Some("Open Home to attach from Library.".to_string()));
                    let _ = attach_trigger_app.render();
                }
                Err(error) => {
                    attach_trigger_app.set_error(Some(js_error(error)));
                    let _ = attach_trigger_app.render();
                }
            }
        }));
        self.attach_button
            .add_event_listener_with_callback("click", attach_click.as_ref().unchecked_ref())?;
        attach_click.forget();

        let toggle_app = Rc::clone(self);
        let toggle_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            {
                let mut state = toggle_app.state.borrow_mut();
                state.show_participants = !state.show_participants;
            }
            let _ = toggle_app.render();
        }));
        self.participant_toggle
            .add_event_listener_with_callback("click", toggle_click.as_ref().unchecked_ref())?;
        toggle_click.forget();

        let access_toggle_app = Rc::clone(self);
        let access_toggle = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            {
                let mut state = access_toggle_app.state.borrow_mut();
                state.show_access_controls = !state.show_access_controls;
            }
            let _ = access_toggle_app.render();
        }));
        self.room_access_toggle
            .add_event_listener_with_callback("click", access_toggle.as_ref().unchecked_ref())?;
        access_toggle.forget();

        let close_roster_app = Rc::clone(self);
        let close_roster = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            {
                let mut state = close_roster_app.state.borrow_mut();
                state.show_participants = false;
            }
            let _ = close_roster_app.render();
        }));
        self.participant_close
            .add_event_listener_with_callback("click", close_roster.as_ref().unchecked_ref())?;
        self.participant_scrim
            .add_event_listener_with_callback("click", close_roster.as_ref().unchecked_ref())?;
        close_roster.forget();

        let browser_access_app = Rc::clone(self);
        let browser_access_click =
            Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
                let Some(target) = event
                    .target()
                    .and_then(|value| value.dyn_into::<Element>().ok())
                else {
                    return;
                };
                let Some(button) = target
                    .closest("[data-browser-access-action]")
                    .ok()
                    .flatten()
                else {
                    return;
                };
                let Some(action) = button.get_attribute("data-browser-access-action") else {
                    return;
                };
                let Some(request_id) = button.get_attribute("data-request-id") else {
                    return;
                };
                if request_id.trim().is_empty() {
                    return;
                }
                let app = Rc::clone(&browser_access_app);
                spawn_local(async move {
                    app.clear_error();
                    let result = match action.as_str() {
                        "approve" => app.approve_browser_access_request(&request_id).await,
                        "deny" => app.deny_browser_access_request(&request_id).await,
                        _ => Ok(()),
                    };
                    if let Err(err) = result {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }));
        self.browser_access_list.add_event_listener_with_callback(
            "click",
            browser_access_click.as_ref().unchecked_ref(),
        )?;
        browser_access_click.forget();

        let join_invite_app = Rc::clone(self);
        let join_invite_submit =
            Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
                event.prevent_default();
                let app = Rc::clone(&join_invite_app);
                spawn_local(async move {
                    app.clear_error();
                    if let Err(err) = app.join_conversation_from_invite().await {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }));
        self.conversation_join_form
            .add_event_listener_with_callback(
                "submit",
                join_invite_submit.as_ref().unchecked_ref(),
            )?;
        join_invite_submit.forget();

        let create_invite_app = Rc::clone(self);
        let create_invite_click =
            Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                let app = Rc::clone(&create_invite_app);
                spawn_local(async move {
                    app.clear_error();
                    if let Err(err) = app.create_conversation_join_invite().await {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }));
        self.conversation_invite_create
            .add_event_listener_with_callback(
                "click",
                create_invite_click.as_ref().unchecked_ref(),
            )?;
        create_invite_click.forget();

        let copy_invite_app = Rc::clone(self);
        let copy_invite_click =
            Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                let app = Rc::clone(&copy_invite_app);
                spawn_local(async move {
                    app.clear_error();
                    if let Err(err) = app.copy_conversation_join_invite().await {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }));
        self.conversation_invite_copy
            .add_event_listener_with_callback(
                "click",
                copy_invite_click.as_ref().unchecked_ref(),
            )?;
        copy_invite_click.forget();

        let room_access_app = Rc::clone(self);
        let room_access_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            let Some(target) = event
                .target()
                .and_then(|value| value.dyn_into::<Element>().ok())
            else {
                return;
            };
            let app = Rc::clone(&room_access_app);
            if let Some(button) = target.closest("[data-room-policy]").ok().flatten() {
                let Some(policy) = button.get_attribute("data-room-policy") else {
                    return;
                };
                let enabled = button
                    .get_attribute("data-enabled")
                    .map(|value| value == "true")
                    .unwrap_or(false);
                spawn_local(async move {
                    app.clear_error();
                    if let Err(err) = app.update_room_policy(&policy, !enabled).await {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
                return;
            }
            if let Some(button) = target.closest("[data-guest-action]").ok().flatten() {
                let Some(session_id) = button.get_attribute("data-session-id") else {
                    return;
                };
                spawn_local(async move {
                    app.clear_error();
                    if let Err(err) = app.kick_guest_session(&session_id).await {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
                return;
            }
            if let Some(button) = target.closest("[data-node-action]").ok().flatten() {
                let Some(action) = button.get_attribute("data-node-action") else {
                    return;
                };
                let invite_id = button.get_attribute("data-invite-id");
                spawn_local(async move {
                    app.clear_error();
                    let result = match action.as_str() {
                        "revoke-invite" => {
                            match invite_id.ok_or_else(|| "missing invite".to_string()) {
                                Ok(invite_id) => app.revoke_runtime_invite(&invite_id).await,
                                Err(err) => Err(err),
                            }
                        }
                        _ => Ok(()),
                    };
                    if let Err(err) = result {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }
        }));
        self.room_access_section.add_event_listener_with_callback(
            "click",
            room_access_click.as_ref().unchecked_ref(),
        )?;
        room_access_click.forget();

        for (button, (_, emoji)) in self.emoji_buttons.iter().zip(EMOJI_BUTTON_IDS.iter()) {
            let emoji = (*emoji).to_string();
            let emoji_app = Rc::clone(self);
            let emoji_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
                let current = emoji_app.message_input.value();
                let next = if current.trim().is_empty() {
                    emoji.clone()
                } else {
                    format!("{current} {emoji}")
                };
                emoji_app.message_input.set_value(&next);
                let _ = emoji_app.message_input.focus();
            }));
            button
                .add_event_listener_with_callback("click", emoji_click.as_ref().unchecked_ref())?;
            emoji_click.forget();
        }

        let link_app = Rc::clone(self);
        let link_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            let Some(target) = event
                .target()
                .and_then(|value| value.dyn_into::<Element>().ok())
            else {
                return;
            };
            if let Some(button) = target.closest("[data-open-attachment]").ok().flatten() {
                let Some(attachment_id) = button.get_attribute("data-open-attachment") else {
                    return;
                };
                match link_app.open_attachment_in_documents(&attachment_id) {
                    Ok(true) => {
                        event.prevent_default();
                        link_app.set_status(
                            "Opening",
                            "Opening attachment in Documents inside ElastOS.",
                        );
                        let _ = link_app.render();
                    }
                    Ok(false) => {}
                    Err(error) => {
                        link_app.set_error(Some(js_error(error)));
                        let _ = link_app.render();
                    }
                }
                return;
            }
            let Some(anchor) = target.closest("[data-open-uri]").ok().flatten() else {
                return;
            };
            let Some(uri) = anchor.get_attribute("data-open-uri") else {
                return;
            };
            match link_app.open_elastos_uri_from_shell(&uri) {
                Ok(true) => event.prevent_default(),
                Ok(false) => {}
                Err(error) => {
                    link_app.set_error(Some(js_error(error)));
                    let _ = link_app.render();
                }
            }
        }));
        self.message_list
            .add_event_listener_with_callback("click", link_click.as_ref().unchecked_ref())?;
        link_click.forget();

        let library_attach_app = Rc::clone(self);
        let library_attach =
            Closure::<dyn FnMut(MessageEvent)>::wrap(Box::new(move |event: MessageEvent| {
                let Some(window) = window() else {
                    return;
                };
                let Ok(origin) = window.location().origin() else {
                    return;
                };
                if event.origin() != origin {
                    return;
                }
                let data = event.data();
                if js_string_field(&data, "type").as_deref()
                    != Some("chat-room:attach-library-item")
                {
                    return;
                }
                let blob = js_blob_field(&data, "blob");
                if blob.is_none() {
                    return;
                }
                let file_name = js_string_field(&data, "fileName")
                    .or_else(|| js_string_field(&data, "title"))
                    .unwrap_or_else(|| "Library item".to_string());
                let mime_type = js_string_field(&data, "mimeType")
                    .unwrap_or_else(|| "application/octet-stream".to_string());
                let app = Rc::clone(&library_attach_app);
                spawn_local(async move {
                    app.clear_error();
                    let result = if let Some(blob) = blob {
                        app.send_library_attachment(&file_name, &mime_type, blob)
                            .await
                    } else {
                        Err("Library attachment is missing file bytes.".to_string())
                    };
                    if let Err(err) = result {
                        app.set_error(Some(err));
                    }
                    let _ = app.render();
                });
            }));
        window()
            .ok_or_else(|| JsValue::from_str("window unavailable"))?
            .add_event_listener_with_callback("message", library_attach.as_ref().unchecked_ref())?;
        library_attach.forget();

        let attachment_result_app = Rc::clone(self);
        let attachment_result =
            Closure::<dyn FnMut(MessageEvent)>::wrap(Box::new(move |event: MessageEvent| {
                let Some(window) = window() else {
                    return;
                };
                let Ok(origin) = window.location().origin() else {
                    return;
                };
                if event.origin() != origin {
                    return;
                }
                let data = event.data();
                if js_string_field(&data, "type").as_deref()
                    != Some("chat-room:attachment-open-result")
                {
                    return;
                }
                let ok = Reflect::get(&data, &JsValue::from_str("ok"))
                    .ok()
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                let message = js_string_field(&data, "message").unwrap_or_else(|| {
                    if ok {
                        "Opened attachment in Documents.".to_string()
                    } else {
                        "Documents could not open the attachment.".to_string()
                    }
                });
                if ok {
                    attachment_result_app.set_status("Documents", &message);
                } else {
                    attachment_result_app.set_error(Some(message));
                }
                let _ = attachment_result_app.render();
            }));
        window()
            .ok_or_else(|| JsValue::from_str("window unavailable"))?
            .add_event_listener_with_callback(
                "message",
                attachment_result.as_ref().unchecked_ref(),
            )?;
        attachment_result.forget();

        if self.is_shell_mode() {
            let runtime_events_app = Rc::clone(self);
            let runtime_events =
                Closure::<dyn FnMut(MessageEvent)>::wrap(Box::new(move |event: MessageEvent| {
                    if !runtime_event_is_chat_room(&event) {
                        return;
                    }
                    let app = Rc::clone(&runtime_events_app);
                    spawn_local(async move {
                        app.poll_and_render_once().await;
                    });
                }));
            window()
                .ok_or_else(|| JsValue::from_str("window unavailable"))?
                .add_event_listener_with_callback(
                    "message",
                    runtime_events.as_ref().unchecked_ref(),
                )?;
            runtime_events.forget();
        }

        let close_lifecycle_app = Rc::clone(self);
        let close_lifecycle = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            close_lifecycle_app.leave_shell_session_on_close();
        }));
        let window = window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
        window.add_event_listener_with_callback(
            "pagehide",
            close_lifecycle.as_ref().unchecked_ref(),
        )?;
        window.add_event_listener_with_callback(
            "beforeunload",
            close_lifecycle.as_ref().unchecked_ref(),
        )?;
        close_lifecycle.forget();

        Ok(())
    }

    fn clear_error(&self) {
        let mut state = self.state.borrow_mut();
        state.error_text = None;
        state.error_transient = false;
    }

    fn clear_error_if(&self, expected: &str) {
        let mut state = self.state.borrow_mut();
        if state.error_text.as_deref() != Some(expected) {
            return;
        }
        state.error_text = None;
        state.error_transient = false;
    }

    fn set_error(&self, error: Option<String>) {
        let mut state = self.state.borrow_mut();
        state.error_text = error;
        state.error_transient = false;
    }

    fn set_status(&self, badge: &str, detail: &str) {
        let mut state = self.state.borrow_mut();
        state.status_badge = badge.to_string();
        state.status_detail = detail.to_string();
        state.error_text = None;
        state.error_transient = false;
    }

    fn set_transient_error(&self, error: Option<String>) {
        let mut state = self.state.borrow_mut();
        state.error_text = error;
        state.error_transient = state.error_text.is_some();
    }

    fn session_loss_detail(&self) -> &'static str {
        if self.is_shell_mode() {
            "The local conversation session ended. Open Chat again to reconnect."
        } else {
            "This browser was removed from the conversation."
        }
    }

    fn leave_shell_session_on_close(&self) {
        if !self.is_shell_mode() {
            return;
        }
        let should_leave = {
            let mut state = self.state.borrow_mut();
            if !state.session_active || state.close_leave_sent {
                false
            } else {
                state.close_leave_sent = true;
                true
            }
        };
        if !should_leave {
            return;
        }
        let _ = send_keepalive_post(
            &self.room_api_url("/session/leave"),
            &self.home_token_headers(),
        );
    }

    fn restore_session(self: &Rc<Self>) {
        let should_restore = {
            let state = self.state.borrow();
            state.request_id.is_none() && !state.session_active
        };
        if !should_restore {
            return;
        }

        let app = Rc::clone(self);
        spawn_local(async move {
            let headers = app.room_request_headers();
            match api_post_session_json(
                &app.room_api_url("/poll"),
                &RoomPollInput { since: 0 },
                &headers,
            )
            .await
            {
                Ok(poll) => {
                    let (attachments, _changed) = app.apply_active_poll(poll);
                    for attachment in attachments {
                        if let Err(err) = app.cache_attachment_data_url(&attachment).await {
                            app.set_error(Some(err));
                            break;
                        }
                    }
                    let _ = app.render();
                }
                Err(err) if is_session_error(&err) => {}
                Err(err) => {
                    app.set_transient_error(Some(err));
                    let _ = app.render();
                }
            }
        });
    }

    fn apply_active_poll(&self, poll: RoomPollView) -> (Vec<AttachmentView>, bool) {
        apply_active_poll_state(&mut self.state.borrow_mut(), poll)
    }

    fn clear_browser_session_request_storage(&self) {
        if let Some(storage) = &self.session_storage {
            storage
                .remove_item(&self.config.browser_session_request_storage_key)
                .ok();
        }
    }

    fn store_browser_session_request(&self, request_id: &str) -> Result<(), String> {
        let storage = self
            .session_storage
            .as_ref()
            .ok_or_else(|| "Browser session storage unavailable.".to_string())?;
        storage
            .set_item(&self.config.browser_session_request_storage_key, request_id)
            .map_err(js_error)
    }

    fn reset_browser_session_request(&self) {
        self.clear_browser_session_request_storage();
        if let Some(gateway_ui) = &self.gateway_ui {
            gateway_ui.display_name_input.set_value("");
        }
        let mut state = self.state.borrow_mut();
        state.request_id = None;
        state.session_active = false;
        state.show_participants = false;
        state.show_access_controls = false;
        state.force_message_follow = true;
        state.display_name.clear();
        state.latest_seq = 0;
        state.objects.clear();
        state.participants.clear();
        state.active_sessions.clear();
        state.attachment_urls.clear();
        state.status_badge = self.default_status_badge();
        state.status_detail = self.default_status_detail();
        state.error_text = None;
        state.error_transient = false;
    }

    fn handle_session_loss(&self, detail: &str) {
        self.clear_browser_session_request_storage();
        let mut state = self.state.borrow_mut();
        state.request_id = None;
        state.session_active = false;
        state.show_participants = false;
        state.show_access_controls = false;
        state.force_message_follow = true;
        state.latest_seq = 0;
        state.objects.clear();
        state.participants.clear();
        state.active_sessions.clear();
        state.attachment_urls.clear();
        state.status_badge = if self.is_shell_mode() {
            "Reconnect".to_string()
        } else {
            "Join".to_string()
        };
        state.status_detail = detail.to_string();
        state.error_text = None;
        state.error_transient = false;
    }

    fn hydrate_defaults(self: &Rc<Self>) {
        let should_fetch = {
            let state = self.state.borrow();
            state.display_name.trim().is_empty()
                && state.request_id.is_none()
                && !state.session_active
        };
        if !should_fetch {
            return;
        }

        let app = Rc::clone(self);
        spawn_local(async move {
            let requested_direct = app.config.initial_direct_conversation_id.clone();
            let bootstrap_generation = app.state.borrow().selection_generation;
            let direct_loaded = app.refresh_direct_conversations().await.is_ok();
            if let Some(conversation_id) = requested_direct {
                let decision = requested_conversation_decision(
                    direct_loaded,
                    &app.state.borrow().direct.conversations,
                    &conversation_id,
                );
                match decision {
                    RequestedConversationDecision::Available => {
                        let selection_guard = {
                            let mut state = app.state.borrow_mut();
                            commit_requested_direct_selection_if_current(
                                &mut state,
                                bootstrap_generation,
                                &conversation_id,
                            )
                        };
                        if let Some(selection_guard) = selection_guard {
                            match app
                                .refresh_direct_messages_for_guard(&selection_guard)
                                .await
                            {
                                Ok(_) => {
                                    if app.selection_guard_is_current(&selection_guard) {
                                        app.start_poll_loop();
                                        let _ = app.render();
                                        return;
                                    }
                                }
                                Err(403) if app.selection_guard_is_current(&selection_guard) => {
                                    app.return_to_conversation_selector(Some(&conversation_id));
                                }
                                Err(_) if app.selection_guard_is_current(&selection_guard) => {
                                    app.set_error(Some(
                                        "Direct messages are temporarily unavailable.".to_string(),
                                    ));
                                }
                                Err(_) => {}
                            }
                        }
                    }
                    RequestedConversationDecision::Unavailable => {
                        app.return_to_conversation_selector(Some(&conversation_id));
                    }
                    RequestedConversationDecision::TemporarilyUnavailable => {
                        app.set_transient_error(Some(
                            "Direct conversations are temporarily unavailable.".to_string(),
                        ));
                    }
                }
            }
            let summary = match api_get_json_with_headers::<SummaryView>(
                &app.room_api_url("/summary"),
                &app.home_token_headers(),
            )
            .await
            {
                Ok(summary) => summary,
                Err(error) => {
                    if app.is_shell_mode() {
                        app.set_error(Some(
                            ShellSessionBootstrapFailure::from_request_error(&error)
                                .detail()
                                .to_string(),
                        ));
                        let _ = app.render();
                    }
                    return;
                }
            };
            let _ = app.apply_summary(&summary);
            if app.is_shell_mode() {
                let shared_guard = {
                    let state = app.state.borrow();
                    current_selection_guard(&state)
                };
                let bootstrap = if let Some(invite) = app.config.initial_join_invite.as_deref() {
                    app.conversation_join_input.set_value(invite);
                    app.join_conversation_from_invite().await
                } else {
                    app.ensure_shell_session_for_guard(&shared_guard)
                        .await
                        .map(|_| ())
                };
                if let Err(err) = bootstrap {
                    if app.selection_guard_is_current(&shared_guard) {
                        app.set_error(Some(err));
                    }
                } else if app.selection_guard_is_current(&shared_guard)
                    && app.state.borrow().session_active
                {
                    app.start_poll_loop();
                }
            }
            let _ = app.render();
        });
    }

    async fn refresh_shell_summary_for_guard(
        &self,
        guard: &SelectionGuard,
    ) -> Result<bool, String> {
        if !self.is_shell_mode() {
            return Ok(false);
        }
        let summary = api_get_json_with_headers::<SummaryView>(
            &self.room_api_url("/summary"),
            &self.home_token_headers(),
        )
        .await?;
        if !self.selection_guard_is_current(guard) {
            return Ok(false);
        }
        let mut state = self.state.borrow_mut();
        Ok(
            apply_summary_if_current(&mut state, guard, &summary, self.config.access_mode)
                .unwrap_or(false),
        )
    }

    async fn submit_browser_session_request(&self) -> Result<(), String> {
        let already_active_or_pending = {
            let state = self.state.borrow();
            state.request_id.is_some() || state.session_active
        };
        if already_active_or_pending {
            return Ok(());
        }

        let gateway_ui = self
            .gateway_ui
            .as_ref()
            .ok_or_else(|| "internal error: join controls missing".to_string())?;

        let mut display_name = gateway_ui.display_name_input.value().trim().to_string();
        let summary = api_get_json_with_headers::<SummaryView>(
            &self.room_api_url("/summary"),
            &self.home_token_headers(),
        )
        .await
        .ok();
        if let Some(summary) = &summary {
            let _ = self.apply_summary(summary);
            if !summary.browser_access_allowed {
                return Err(summary
                    .browser_access_block_reason
                    .clone()
                    .unwrap_or_else(|| {
                        "This device cannot approve web guest access to this conversation."
                            .to_string()
                    }));
            }
        }
        if display_name.is_empty() {
            display_name = {
                let state = self.state.borrow();
                state.display_name.trim().to_string()
            };
        }
        if display_name.is_empty() {
            return Err(DISPLAY_NAME_REQUIRED_ERROR.to_string());
        }

        let payload = BrowserSessionRequestInput {
            display_name: &display_name,
            device_label: "",
            capabilities: &ROOM_ACCESS_CAPABILITIES,
        };
        let response: BrowserSessionRequestOutput =
            api_post_json(&format!("{BROWSER_SESSION_API_BASE}/request"), &payload).await?;

        self.store_browser_session_request(&response.request_id)?;

        let mut state = self.state.borrow_mut();
        state.request_id = Some(response.request_id);
        state.display_name = display_name;
        state.status_badge = APPROVAL_REQUESTED_BADGE.to_string();
        state.status_detail = APPROVAL_REQUESTED_DETAIL.to_string();
        Ok(())
    }

    async fn ensure_shell_session_for_guard(&self, guard: &SelectionGuard) -> Result<bool, String> {
        if !self.is_shell_mode() || self.state.borrow().session_active {
            return Ok(false);
        }

        let summary = api_get_json_with_headers::<SummaryView>(
            &self.room_api_url("/summary"),
            &self.home_token_headers(),
        )
        .await?;
        if !self.selection_guard_is_current(guard) {
            return Ok(false);
        }
        {
            let mut state = self.state.borrow_mut();
            let Some(_changed) =
                apply_summary_if_current(&mut state, guard, &summary, self.config.access_mode)
            else {
                return Ok(false);
            };
        }
        if !shell_summary_allows_session(&summary) {
            let detail = summary
                .browser_access_block_reason
                .clone()
                .unwrap_or_else(|| SHELL_ACCESS_UNAVAILABLE_DETAIL.to_string());
            let mut state = self.state.borrow_mut();
            if !selection_guard_matches(&state, guard) {
                return Ok(false);
            }
            state.error_text = Some(detail.clone());
            state.error_transient = false;
            state.status_badge = "Conversation unavailable".to_string();
            state.status_detail = trim_sentence(&detail);
            return Ok(true);
        }

        let start: ShellSessionStartOutput = api_post_empty_json_with_headers(
            &self.room_api_url("/session/start"),
            &self.home_token_headers(),
        )
        .await
        .map_err(|error| {
            ShellSessionBootstrapFailure::from_request_error(&error)
                .detail()
                .to_string()
        })?;
        if !self.selection_guard_is_current(guard) {
            return Ok(false);
        }
        let (attachments_to_cache, changed) = {
            let mut state = self.state.borrow_mut();
            let Some(result) = apply_active_poll_if_current(&mut state, guard, start.poll) else {
                return Ok(false);
            };
            result
        };
        for attachment in attachments_to_cache {
            if !self.selection_guard_is_current(guard) {
                return Ok(changed);
            }
            self.cache_attachment_data_url(&attachment).await?;
        }
        Ok(changed)
    }

    async fn approve_browser_access_request(&self, request_id: &str) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("web guest approval is only available from Home".to_string());
        }
        let _: serde_json::Value = api_post_empty_json_with_headers(
            &self.room_api_url(&format!("/requests/{request_id}/approve")),
            &self.home_token_headers(),
        )
        .await?;
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        Ok(())
    }

    async fn deny_browser_access_request(&self, request_id: &str) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("web guest denial is only available from Home".to_string());
        }
        let _: serde_json::Value = api_post_empty_json_with_headers(
            &self.room_api_url(&format!("/requests/{request_id}/deny")),
            &self.home_token_headers(),
        )
        .await?;
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        Ok(())
    }

    async fn kick_guest_session(&self, session_id: &str) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("web guest removal is only available from Home".to_string());
        }
        let _: serde_json::Value = api_post_empty_json_with_headers(
            &self.room_api_url(&format!("/guests/{session_id}/kick")),
            &self.home_token_headers(),
        )
        .await?;
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        Ok(())
    }

    async fn update_room_policy(&self, policy: &str, enabled: bool) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("conversation access controls are only available from Home".to_string());
        }
        let current = {
            let state = self.state.borrow();
            state.room_control.access_policy.clone()
        };
        let payload = RoomAccessPolicyInput {
            allow_guest_invites: if policy == "guest" {
                enabled
            } else {
                current.allow_guest_invites
            },
            allow_member_invites: if policy == "member" {
                enabled
            } else {
                current.allow_member_invites
            },
            allow_members_to_host_guests: if policy == "host" {
                enabled
            } else {
                current.allow_members_to_host_guests
            },
        };
        let _: serde_json::Value = api_post_json_with_headers(
            &self.room_api_url("/access-policy"),
            &payload,
            &self.home_token_headers(),
        )
        .await?;
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        Ok(())
    }

    async fn create_conversation_join_invite(&self) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("conversation links are only available from Home".to_string());
        }
        let invite: ConversationJoinInviteView = api_post_json_with_headers(
            &self.room_api_url("/invites/create-link"),
            &ConversationJoinInviteCreateInput {},
            &self.home_token_headers(),
        )
        .await?;
        {
            let mut state = self.state.borrow_mut();
            state.join_invite_url = Some(invite.invite_url);
        }
        self.set_status(
            "Invite ready",
            "Share this link with another ElastOS device.",
        );
        Ok(())
    }

    async fn copy_conversation_join_invite(&self) -> Result<(), String> {
        let invite_url = {
            let state = self.state.borrow();
            state
                .join_invite_url
                .clone()
                .filter(|value| !value.trim().is_empty())
        }
        .ok_or_else(|| "Create a join link first.".to_string())?;
        copy_text_to_clipboard(&invite_url).await?;
        self.set_status("Copied", "Conversation join link copied.");
        Ok(())
    }

    async fn join_conversation_from_invite(&self) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("conversation links must be opened from Home".to_string());
        }
        let invite = self.conversation_join_input.value().trim().to_string();
        if invite.is_empty() {
            return Err("Paste a conversation invite code or link.".to_string());
        }
        self.set_status("Joining", "Claiming the invite and connecting this device.");
        let joined: ConversationJoinInviteJoinResponse = api_post_json_with_headers(
            &self.room_api_url("/invites/join"),
            &ConversationJoinInviteJoinInput { invite: &invite },
            &self.home_token_headers(),
        )
        .await?;
        self.conversation_join_input.set_value("");
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        let _ = self
            .ensure_shell_session_for_guard(&selection_guard)
            .await?;
        self.set_status("Joined", &format!("Joined {}.", joined.room_title));
        Ok(())
    }

    async fn revoke_runtime_invite(&self, invite_id: &str) -> Result<(), String> {
        if !self.is_shell_mode() {
            return Err("ElastOS invite cancellation is only available from Home".to_string());
        }
        let _: serde_json::Value = api_post_json_with_headers(
            &self.room_api_url("/invites/revoke"),
            &RoomInviteRevokeInput { invite_id },
            &self.home_token_headers(),
        )
        .await?;
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_shell_summary_for_guard(&selection_guard)
            .await?;
        Ok(())
    }

    async fn send_message(&self) -> Result<(), String> {
        if self.is_direct_mode() {
            return self.send_direct_message().await;
        }
        if !self.state.borrow().session_active {
            return Ok(());
        }
        let body = self.message_input.value().trim().to_string();
        if body.is_empty() {
            return Ok(());
        }

        let request_id = {
            let mut state = self.state.borrow_mut();
            pending_chat_request_id(
                &mut state.pending_chat_send,
                &body,
                new_chat_message_request_id,
            )?
        };
        if self.send_room_text(&request_id, &body).await? {
            let mut state = self.state.borrow_mut();
            if state
                .pending_chat_send
                .as_ref()
                .is_some_and(|pending| pending.request_id == request_id && pending.body == body)
            {
                state.pending_chat_send = None;
            }
            drop(state);
            self.message_input.set_value("");
        }
        Ok(())
    }

    async fn send_direct_message(&self) -> Result<(), String> {
        let conversation_id = self
            .state
            .borrow()
            .direct
            .selected_conversation_id
            .clone()
            .ok_or_else(|| "Choose a conversation first.".to_string())?;
        if selected_conversation(&self.state.borrow().direct.conversations, &conversation_id)
            .is_none()
        {
            self.return_to_conversation_selector(Some(&conversation_id));
            return Err("That conversation is no longer available.".to_string());
        }
        let text = self.message_input.value().trim().to_string();
        if text.is_empty() {
            return Ok(());
        }
        let request_id = {
            let mut state = self.state.borrow_mut();
            pending_direct_request_id(
                &mut state.direct.pending_send,
                &conversation_id,
                &text,
                new_chat_message_request_id,
            )?
        };
        let payload = DirectSendInput {
            request_id: &request_id,
            conversation_id: &conversation_id,
            text: &text,
        };
        let response_result: Result<(u16, DirectSendResponse), u16> = direct_post_json(
            &format!("{DIRECT_API_BASE}/messages/send"),
            &payload,
            &self.home_token_headers(),
        )
        .await;
        let current_selection = self.state.borrow().direct.selected_conversation_id.clone();
        if current_selection.as_deref() != Some(conversation_id.as_str()) {
            return Ok(());
        }
        let (http_status, response) = match response_result {
            Ok(response) => response,
            Err(403) => {
                self.return_to_conversation_selector(Some(&conversation_id));
                return Err("That conversation is no longer available.".to_string());
            }
            Err(_) => return Err("Message could not be sent. Try again.".to_string()),
        };
        if !valid_direct_send_response(http_status, response.status) {
            return Err("Message could not be sent. Try again.".to_string());
        }
        {
            let mut state = self.state.borrow_mut();
            if state.direct.pending_send.as_ref().is_some_and(|pending| {
                pending.request_id == request_id
                    && pending.conversation_id == conversation_id
                    && pending.text == text
            }) {
                state.direct.pending_send = None;
            }
        }
        self.message_input.set_value("");
        let selection_guard = {
            let state = self.state.borrow();
            current_selection_guard(&state)
        };
        let _ = self
            .refresh_direct_messages_for_guard(&selection_guard)
            .await;
        Ok(())
    }

    async fn send_library_attachment(
        &self,
        file_name: &str,
        mime_type: &str,
        blob: Blob,
    ) -> Result<(), String> {
        let bytes = blob_to_bytes(blob).await?;
        if bytes.is_empty() {
            return Err("Library item is empty.".to_string());
        }
        self.send_attachment_bytes(file_name, mime_type, &bytes)
            .await
    }

    async fn send_attachment_bytes(
        &self,
        file_name: &str,
        mime_type: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        if !self.state.borrow().session_active {
            return Ok(());
        }
        let headers = self.room_request_headers();
        let start: AttachmentUploadStartResponse = api_post_session_json(
            &self.room_api_url("/upload/start"),
            &AttachmentUploadStartInput {
                file_name,
                mime_type,
                size_bytes: bytes.len() as u64,
            },
            &headers,
        )
        .await?;
        let chunk_size = usize::try_from(start.chunk_size_bytes)
            .ok()
            .filter(|value| *value > 0)
            .unwrap_or(256 * 1024);
        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = (offset + chunk_size).min(bytes.len());
            let mut chunk_headers = headers.clone();
            chunk_headers.push(("x-elastos-upload-offset", offset.to_string()));
            let _: serde_json::Value = api_post_session_bytes(
                &self.room_api_url(&format!("/upload/{}/chunk", start.upload_id)),
                &bytes[offset..end],
                &chunk_headers,
            )
            .await?;
            offset = end;
        }
        let sent: ConversationObjectView = api_post_empty_json_with_headers(
            &self.room_api_url(&format!("/upload/{}/finish", start.upload_id)),
            &headers,
        )
        .await?;

        let mut state = self.state.borrow_mut();
        state.latest_seq = sent.seq;
        state.objects.push(sent);
        state.force_message_follow = true;
        Ok(())
    }

    async fn send_room_text(&self, request_id: &str, body: &str) -> Result<bool, String> {
        if !self.state.borrow().session_active {
            return Ok(false);
        }
        let payload = SendMessageInput { request_id, body };
        let headers = self.room_request_headers();
        let sent: ConversationObjectView =
            match api_post_session_json(&self.room_api_url("/objects/send"), &payload, &headers)
                .await
            {
                Ok(sent) => sent,
                Err(err) if is_session_error(&err) => {
                    self.handle_session_loss(self.session_loss_detail());
                    return Ok(false);
                }
                Err(err) => return Err(err),
            };

        let mut state = self.state.borrow_mut();
        state.latest_seq = sent.seq;
        state.objects.push(sent);
        state.force_message_follow = true;
        Ok(true)
    }

    fn open_elastos_uri_from_shell(&self, uri: &str) -> Result<bool, JsValue> {
        let clean_uri = uri.trim();
        let Some(home_token) = self.config.home_token.as_deref() else {
            return Ok(false);
        };
        if !self.is_shell_mode() || !clean_uri.starts_with("elastos://") {
            return Ok(false);
        }
        let Some(window) = window() else {
            return Ok(false);
        };
        let Some(parent) = window.parent()? else {
            return Ok(false);
        };
        if JsObject::is(parent.as_ref(), window.as_ref()) {
            return Ok(false);
        }

        let message = JsObject::new();
        Reflect::set(
            &message,
            &JsValue::from_str("type"),
            &JsValue::from_str("home:open-uri"),
        )?;
        Reflect::set(
            &message,
            &JsValue::from_str("uri"),
            &JsValue::from_str(clean_uri),
        )?;
        Reflect::set(
            &message,
            &JsValue::from_str("preferredViewer"),
            &JsValue::from_str("documents"),
        )?;
        Reflect::set(
            &message,
            &JsValue::from_str("homeToken"),
            &JsValue::from_str(home_token),
        )?;
        parent.post_message(&message.into(), &window.location().origin()?)?;
        Ok(true)
    }

    fn open_attachment_in_documents(&self, attachment_id: &str) -> Result<bool, JsValue> {
        let clean_attachment_id = attachment_id.trim();
        let Some(home_token) = self.config.home_token.as_deref() else {
            return Err(JsValue::from_str(
                "Open Chat from Home to open attachments in Documents.",
            ));
        };
        if !self.is_shell_mode() {
            return Err(JsValue::from_str(
                "Open Chat from Home to open attachments in Documents.",
            ));
        }
        if clean_attachment_id.is_empty() {
            return Err(JsValue::from_str("Attachment id is missing."));
        }
        let (attachment, data_url) = {
            let state = self.state.borrow();
            let attachment = state
                .objects
                .iter()
                .filter_map(|object| object.attachment.as_ref())
                .find(|attachment| attachment.attachment_id == clean_attachment_id)
                .cloned()
                .ok_or_else(|| JsValue::from_str("Attachment not found."))?;
            let data_url = state
                .attachment_urls
                .get(clean_attachment_id)
                .cloned()
                .ok_or_else(|| JsValue::from_str("Attachment bytes are still loading."))?;
            (attachment, data_url)
        };
        let Some(window) = window() else {
            return Err(JsValue::from_str("Browser window is unavailable."));
        };
        let Some(parent) = window.parent()? else {
            return Err(JsValue::from_str("Home shell is unavailable."));
        };
        if JsObject::is(parent.as_ref(), window.as_ref()) {
            return Err(JsValue::from_str(
                "Open Chat from Home to open attachments in Documents.",
            ));
        }

        let payload = JsObject::new();
        Reflect::set(
            &payload,
            &JsValue::from_str("type"),
            &JsValue::from_str("documents:open-chat-attachment"),
        )?;
        Reflect::set(
            &payload,
            &JsValue::from_str("attachmentId"),
            &JsValue::from_str(&attachment.attachment_id),
        )?;
        Reflect::set(
            &payload,
            &JsValue::from_str("fileName"),
            &JsValue::from_str(&attachment.file_name),
        )?;
        Reflect::set(
            &payload,
            &JsValue::from_str("mimeType"),
            &JsValue::from_str(&attachment.mime_type),
        )?;
        Reflect::set(
            &payload,
            &JsValue::from_str("sizeBytes"),
            &JsValue::from_f64(attachment.size_bytes as f64),
        )?;
        Reflect::set(
            &payload,
            &JsValue::from_str("dataUrl"),
            &JsValue::from_str(&data_url),
        )?;

        let message = JsObject::new();
        Reflect::set(
            &message,
            &JsValue::from_str("type"),
            &JsValue::from_str("home:open-target-with-payload"),
        )?;
        Reflect::set(
            &message,
            &JsValue::from_str("target"),
            &JsValue::from_str("documents"),
        )?;
        Reflect::set(&message, &JsValue::from_str("payload"), &payload)?;
        Reflect::set(
            &message,
            &JsValue::from_str("homeToken"),
            &JsValue::from_str(home_token),
        )?;
        parent.post_message(&message.into(), &window.location().origin()?)?;
        Ok(true)
    }

    fn open_library_from_home(&self) -> Result<bool, JsValue> {
        let Some(home_token) = self.config.home_token.as_deref() else {
            return Ok(false);
        };
        let Some(window) = window() else {
            return Ok(false);
        };
        let Some(parent) = window.parent()? else {
            return Ok(false);
        };
        if JsObject::is(parent.as_ref(), window.as_ref()) {
            return Ok(false);
        }

        let message = JsObject::new();
        Reflect::set(
            &message,
            &JsValue::from_str("type"),
            &JsValue::from_str("home:open-target"),
        )?;
        Reflect::set(
            &message,
            &JsValue::from_str("target"),
            &JsValue::from_str("library"),
        )?;
        let query = JsObject::new();
        Reflect::set(
            &query,
            &JsValue::from_str("mode"),
            &JsValue::from_str("attach"),
        )?;
        Reflect::set(
            &query,
            &JsValue::from_str("returnTarget"),
            &JsValue::from_str("chat-room"),
        )?;
        Reflect::set(&message, &JsValue::from_str("query"), &query)?;
        Reflect::set(
            &message,
            &JsValue::from_str("homeToken"),
            &JsValue::from_str(home_token),
        )?;
        parent.post_message(&message.into(), &window.location().origin()?)?;
        Ok(true)
    }

    async fn poll_once_for_guard(&self, guard: Option<&SelectionGuard>) -> Result<bool, String> {
        let session_active = {
            let state = self.state.borrow();
            state.session_active
        };
        if session_active {
            let headers = self.room_request_headers();
            let poll: RoomPollView = match api_post_session_json(
                &self.room_api_url("/poll"),
                &RoomPollInput {
                    since: {
                        let state = self.state.borrow();
                        state.latest_seq
                    },
                },
                &headers,
            )
            .await
            {
                Ok(poll) => poll,
                Err(err) if is_session_error(&err) => {
                    self.handle_session_loss(self.session_loss_detail());
                    return Ok(true);
                }
                Err(err) => return Err(err),
            };

            if guard
                .is_some_and(|selection_guard| !self.selection_guard_is_current(selection_guard))
            {
                return Ok(false);
            }
            let (attachments_to_cache, changed) = if let Some(selection_guard) = guard {
                let mut state = self.state.borrow_mut();
                let Some(result) = apply_active_poll_if_current(&mut state, selection_guard, poll)
                else {
                    return Ok(false);
                };
                result
            } else {
                self.apply_active_poll(poll)
            };
            for attachment in attachments_to_cache {
                if guard.is_some_and(|selection_guard| {
                    !self.selection_guard_is_current(selection_guard)
                }) {
                    return Ok(changed);
                }
                self.cache_attachment_data_url(&attachment).await?;
            }
            return Ok(changed);
        }

        let request_id = {
            let state = self.state.borrow();
            state.request_id.clone()
        };
        let Some(request_id) = request_id else {
            return Ok(false);
        };

        let status: BrowserSessionStatusOutput = api_get_json_with_headers(
            &format!("{BROWSER_SESSION_API_BASE}/request/{request_id}"),
            &[],
        )
        .await?;

        match status.status.as_str() {
            "pending" => {
                let mut state = self.state.borrow_mut();
                let changed = state.status_badge != APPROVAL_REQUESTED_BADGE
                    || state.status_detail != APPROVAL_REQUESTED_DETAIL;
                state.status_badge = APPROVAL_REQUESTED_BADGE.to_string();
                state.status_detail = APPROVAL_REQUESTED_DETAIL.to_string();
                Ok(changed)
            }
            "approved" => {
                self.clear_browser_session_request_storage();

                let mut state = self.state.borrow_mut();
                let previous_badge = state.status_badge.clone();
                let previous_detail = state.status_detail.clone();
                state.request_id = None;
                state.session_active = true;
                state.status_badge = "Joining".to_string();
                let _ = status.expires_at;
                state.status_detail = "Approved. Opening conversation.".to_string();
                Ok(previous_badge != state.status_badge || previous_detail != state.status_detail)
            }
            "denied" => {
                self.clear_browser_session_request_storage();
                let mut state = self.state.borrow_mut();
                state.request_id = None;
                state.status_badge = "Denied".to_string();
                state.status_detail = status
                    .denial_reason
                    .clone()
                    .unwrap_or_else(|| "This request was denied.".to_string());
                Ok(true)
            }
            "expired" => {
                self.clear_browser_session_request_storage();
                let mut state = self.state.borrow_mut();
                state.request_id = None;
                state.status_badge = "Expired".to_string();
                state.status_detail = "This request expired. Try again.".to_string();
                Ok(true)
            }
            other => {
                let mut state = self.state.borrow_mut();
                state.status_badge = "Unknown".to_string();
                state.status_detail = format!("Unexpected browser session status: {}.", other);
                Ok(true)
            }
        }
    }

    async fn cache_attachment_data_url(&self, attachment: &AttachmentView) -> Result<(), String> {
        {
            let state = self.state.borrow();
            if state
                .attachment_urls
                .contains_key(&attachment.attachment_id)
            {
                return Ok(());
            }
        }

        let headers = self.room_request_headers();
        let bytes = match api_get_session_bytes(
            &self.room_api_url(&format!("/attachments/{}", attachment.attachment_id)),
            &headers,
        )
        .await
        {
            Ok(bytes) => bytes,
            Err(err) if is_session_error(&err) => {
                self.handle_session_loss(self.session_loss_detail());
                return Ok(());
            }
            Err(err) => return Err(err),
        };

        let data_url = format!(
            "data:{};base64,{}",
            attachment.mime_type,
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        self.state
            .borrow_mut()
            .attachment_urls
            .insert(attachment.attachment_id.clone(), data_url);
        Ok(())
    }

    fn render(&self) -> Result<(), JsValue> {
        let follow_messages = should_follow_scroll(&self.message_list);
        let previous_message_scroll_top = self.message_list.scroll_top();
        let previous_participant_scroll_top = self.participant_list.scroll_top();
        let (state, force_message_follow) = {
            let mut state = self.state.borrow_mut();
            let force_message_follow = state.force_message_follow;
            state.force_message_follow = false;
            (state.clone(), force_message_follow)
        };

        if let Some(status_badge) = &self.status_badge {
            status_badge.set_text_content(Some(&state.status_badge));
        }
        if let Some(status_detail) = &self.status_detail {
            status_detail.set_text_content(Some(&state.status_detail));
        }
        let pending = state.request_id.is_some();
        let projection = render_projection(&state, self.is_shell_mode());
        let session_active = projection.session_active;
        let direct_mode = projection.direct_mode;
        let direct_send_enabled = direct_mode
            && state
                .direct
                .selected_conversation_id
                .as_deref()
                .is_some_and(|id| {
                    // A removed relationship is read-only: history renders,
                    // the composer stays dark.
                    selected_conversation(&state.direct.conversations, id)
                        .is_some_and(|conversation| !conversation.removed)
                });
        let controls = chat_control_policy(
            state.room_mode_known,
            state.collaboration_configured,
            self.is_shell_mode(),
            session_active,
            state.show_access_controls,
            !state.pending_requests.is_empty(),
        );
        self.participant_count
            .set_text_content(Some(&projection.participant_count));
        self.browser_access_count
            .set_text_content(Some(&format_browser_access_count(
                state.pending_requests.len(),
            )));
        self.participant_toggle
            .set_text_content(Some(&format_participant_toggle_label(
                state.participants.len(),
                state.show_participants,
            )));
        let show_chat_surface = true;
        self.participant_toggle.set_attribute(
            "aria-expanded",
            if state.show_participants {
                "true"
            } else {
                "false"
            },
        )?;
        self.body.set_attribute(
            "data-room-session-active",
            if session_active { "true" } else { "false" },
        )?;
        self.body.set_attribute(
            "data-chat-mode",
            if direct_mode { "direct" } else { "shared" },
        )?;
        self.chat_card.set_attribute(
            "data-roster-open",
            if state.show_participants && !direct_mode {
                "true"
            } else {
                "false"
            },
        )?;
        set_hidden(
            &self.participant_scrim,
            direct_mode || !state.show_participants,
        )?;
        self.participant_scrim.set_attribute(
            "aria-hidden",
            if direct_mode || !state.show_participants {
                "true"
            } else {
                "false"
            },
        )?;

        if let Some(error) = &state.error_text {
            self.error_text.remove_attribute("hidden")?;
            self.error_text.set_text_content(Some(error));
        } else {
            self.error_text.set_attribute("hidden", "")?;
            self.error_text.set_text_content(None);
        }

        let show_reset = pending
            || !state.display_name.trim().is_empty()
            || matches!(
                state.status_badge.as_str(),
                "Denied" | "Expired" | "Join again"
            );
        if let Some(gateway_ui) = &self.gateway_ui {
            let show_gateway_status = pending
                || !session_active
                    && (!state.browser_access_allowed
                        || state.browser_access_block_reason.is_some()
                        || matches!(
                            state.status_badge.as_str(),
                            "Waiting"
                                | "Joining"
                                | "Denied"
                                | "Expired"
                                | "Join again"
                                | "Unavailable"
                                | "Conversation locked"
                                | "Unknown"
                        ));
            set_hidden(&gateway_ui.browser_access_stage, session_active)?;
            set_hidden(&gateway_ui.browser_access_status_row, !show_gateway_status)?;
            set_hidden(&gateway_ui.reset_button, !show_reset)?;
            gateway_ui
                .display_name_input
                .set_disabled(pending || !state.browser_access_allowed);
            gateway_ui
                .browser_access_submit
                .set_disabled(pending || !state.browser_access_allowed);
        }
        set_hidden(&self.chat_card, !show_chat_surface)?;
        set_hidden(&self.conversation_selector, !self.is_shell_mode())?;
        self.render_conversation_selector(&state.direct)?;
        set_hidden(&self.presence_card, direct_mode)?;
        set_hidden(&self.participant_toggle, direct_mode || !session_active)?;
        set_hidden(&self.participant_close, direct_mode || !session_active)?;
        set_hidden(
            &self.room_access_toggle,
            direct_mode || !controls.show_room_access_toggle,
        )?;
        self.room_access_toggle.set_attribute(
            "aria-expanded",
            if state.show_access_controls {
                "true"
            } else {
                "false"
            },
        )?;
        set_hidden(
            &self.browser_access_section,
            direct_mode || !controls.show_browser_requests,
        )?;
        set_hidden(
            &self.room_access_section,
            direct_mode || !controls.show_room_access,
        )?;
        set_hidden(
            &self.conversation_join_section,
            direct_mode || !controls.show_conversation_join,
        )?;
        self.conversation_join_submit
            .set_disabled(!controls.enable_gateway_controls);
        let invite_url = state.join_invite_url.as_deref().unwrap_or_default();
        self.conversation_invite_output.set_value(invite_url);
        set_hidden(
            &self.conversation_invite_output_row,
            invite_url.trim().is_empty(),
        )?;
        self.conversation_invite_create
            .set_disabled(!controls.enable_gateway_controls);
        self.conversation_invite_copy
            .set_disabled(!controls.enable_gateway_controls || invite_url.trim().is_empty());
        self.message_input.set_disabled(if direct_mode {
            !direct_send_enabled
        } else {
            !controls.enable_text_send
        });
        // Direct conversations are text-only by declared decision, and the
        // control says so where the person meets it — visibly unavailable,
        // never silently missing.
        set_hidden(&self.attach_button, !direct_mode && !controls.show_attach)?;
        self.attach_button
            .set_disabled(direct_mode || !controls.enable_attach);
        if direct_mode {
            self.attach_button.set_title(DIRECT_ATTACHMENTS_UNAVAILABLE);
            self.attach_button
                .set_attribute("aria-label", DIRECT_ATTACHMENTS_UNAVAILABLE)?;
        } else {
            self.attach_button.set_title("");
            self.attach_button.remove_attribute("aria-label")?;
        }
        self.send_button.set_disabled(if direct_mode {
            !direct_send_enabled
        } else {
            !controls.enable_text_send
        });
        for button in &self.emoji_buttons {
            button.set_disabled(if direct_mode {
                !direct_send_enabled
            } else {
                !controls.enable_text_send
            });
        }

        if direct_mode {
            self.participant_list.set_inner_html("");
            self.browser_access_list.set_inner_html("");
            self.room_policy_list.set_inner_html("");
            self.node_list.set_inner_html("");
            self.render_direct_messages(&state.direct)?;
        } else {
            self.render_participants(
                &state.participants,
                projection.participant_count == "Opening conversation",
            )?;
            self.render_browser_access_requests(&state.pending_requests)?;
            self.render_room_access(&state)?;
            self.render_objects(&state.objects, &state.attachment_urls)?;
        }
        restore_scroll_position(&self.participant_list, previous_participant_scroll_top);
        if force_message_follow || follow_messages {
            scroll_to_bottom(&self.message_list);
            schedule_scroll_to_bottom(self.message_list.clone());
        } else {
            restore_scroll_position(&self.message_list, previous_message_scroll_top);
        }
        Ok(())
    }

    fn render_conversation_selector(&self, direct: &DirectUiState) -> Result<(), JsValue> {
        self.conversation_selector.set_inner_html("");
        let shared = self.document.create_element("button")?;
        shared.set_attribute("type", "button")?;
        shared.set_attribute("data-conversation-choice", "shared")?;
        shared.set_attribute("title", "Community")?;
        shared.set_attribute(
            "aria-current",
            if direct.selected_conversation_id.is_none() {
                "true"
            } else {
                "false"
            },
        )?;
        shared.set_class_name(if direct.selected_conversation_id.is_none() {
            "conversation-choice active"
        } else {
            "conversation-choice"
        });
        append_conversation_choice_content(
            &self.document,
            &shared,
            "#",
            "Community",
            "Shared room",
        )?;
        self.conversation_selector.append_child(&shared)?;

        for conversation in &direct.conversations {
            let button = self.document.create_element("button")?;
            button.set_attribute("type", "button")?;
            button.set_attribute("data-conversation-choice", &conversation.conversation_id)?;
            button.set_attribute("title", &conversation.display_name)?;
            let selected = direct.selected_conversation_id.as_deref()
                == Some(conversation.conversation_id.as_str());
            button.set_attribute("aria-current", if selected { "true" } else { "false" })?;
            button.set_class_name(if selected {
                "conversation-choice active"
            } else {
                "conversation-choice"
            });
            append_conversation_choice_content(
                &self.document,
                &button,
                &conversation_initial(&conversation.display_name),
                &conversation.display_name,
                if conversation.removed {
                    "Removed contact"
                } else {
                    "Direct message"
                },
            )?;
            self.conversation_selector.append_child(&button)?;
        }

        let selected = direct
            .selected_conversation_id
            .as_deref()
            .and_then(|id| selected_conversation(&direct.conversations, id));
        let (avatar, title, detail) = selected.map_or(
            ("#".to_string(), "Community".to_string(), "Shared room"),
            |conversation| {
                (
                    conversation_initial(&conversation.display_name),
                    conversation.display_name.clone(),
                    if conversation.removed {
                        "Removed contact"
                    } else {
                        "Direct message"
                    },
                )
            },
        );
        self.conversation_avatar.set_text_content(Some(&avatar));
        self.conversation_title.set_text_content(Some(&title));
        self.conversation_detail.set_text_content(Some(detail));
        Ok(())
    }

    fn render_direct_messages(&self, direct: &DirectUiState) -> Result<(), JsValue> {
        self.message_list.set_inner_html("");
        let Some(conversation_id) = direct.selected_conversation_id.as_deref() else {
            return Ok(());
        };
        let Some(conversation) = selected_conversation(&direct.conversations, conversation_id)
        else {
            return Ok(());
        };
        if direct.messages.is_empty() {
            let empty = self.document.create_element("li")?;
            empty.set_class_name("empty");
            empty.set_text_content(Some("No messages yet."));
            self.message_list.append_child(&empty)?;
            return Ok(());
        }
        for message in &direct.messages {
            let outgoing = message.direction == DirectMessageDirection::Outgoing;
            let item = self.document.create_element("li")?;
            item.set_class_name(if outgoing {
                "message self-message"
            } else {
                "message"
            });
            let meta = self.document.create_element("div")?;
            meta.set_class_name("message-meta");
            let sender = self.document.create_element("span")?;
            sender.set_text_content(Some(if outgoing {
                "You"
            } else {
                &conversation.display_name
            }));
            let detail = self.document.create_element("span")?;
            detail.set_text_content(Some(&format!(
                "{} · {}",
                format_time(message.created_at),
                message.delivery_state.label()
            )));
            meta.append_child(&sender)?;
            meta.append_child(&detail)?;
            let body = self.document.create_element("div")?;
            body.set_class_name("message-body");
            body.set_text_content(Some(&message.text));
            item.append_child(&meta)?;
            item.append_child(&body)?;
            self.message_list.append_child(&item)?;
        }
        Ok(())
    }

    fn render_participants(
        &self,
        participants: &[ParticipantView],
        loading_room: bool,
    ) -> Result<(), JsValue> {
        self.participant_list.set_inner_html("");
        if participants.is_empty() {
            let empty = self.document.create_element("li")?;
            if loading_room {
                empty.set_class_name("empty participant-loading");

                let dots = self.document.create_element("span")?;
                dots.set_class_name("loading-dots");
                dots.set_attribute("aria-hidden", "true")?;
                for _ in 0..3 {
                    let dot = self.document.create_element("span")?;
                    dot.set_class_name("loading-dot");
                    dots.append_child(&dot)?;
                }

                let content = self.document.create_element("div")?;
                content.set_class_name("participant-content");

                let title = self.document.create_element("div")?;
                title.set_class_name("participant-name");
                title.set_text_content(Some("Opening"));

                let detail = self.document.create_element("div")?;
                detail.set_class_name("participant-detail");
                detail.set_text_content(Some("Loading chat."));

                content.append_child(&title)?;
                content.append_child(&detail)?;
                empty.append_child(&dots)?;
                empty.append_child(&content)?;
            } else {
                empty.set_class_name("empty");
                empty.set_text_content(Some("No one is here yet."));
            }
            self.participant_list.append_child(&empty)?;
            return Ok(());
        }

        for participant in participants {
            let item = self.document.create_element("li")?;
            let is_local = participant.is_current_session;
            item.set_class_name(if is_local {
                "participant participant-local"
            } else {
                "participant"
            });

            let shown_name = participant_shown_name(participant);
            let avatar = self.document.create_element("div")?;
            avatar.set_class_name("participant-avatar");
            avatar.set_text_content(Some(&participant_initial(shown_name)));

            let content = self.document.create_element("div")?;
            content.set_class_name("participant-content");

            let header = self.document.create_element("div")?;
            header.set_class_name("participant-header");

            let name = self.document.create_element("div")?;
            name.set_class_name("participant-name");
            name.set_text_content(Some(shown_name));
            header.append_child(&name)?;

            let badge_text = if is_local {
                Some("You".to_string())
            } else if let Some(role) = participant.role.as_deref() {
                Some(participant_role_badge(role))
            } else if participant.local_session_count > 0 {
                Some("Guest".to_string())
            } else {
                None
            };
            if let Some(badge_text) = badge_text {
                let badge = self.document.create_element("span")?;
                badge.set_class_name("participant-badge");
                badge.set_text_content(Some(&badge_text));
                header.append_child(&badge)?;
            }

            let detail = self.document.create_element("div")?;
            detail.set_class_name("participant-detail");
            detail.set_text_content(Some(&participant_detail(
                participant,
                is_local,
                self.is_shell_mode(),
            )));

            content.append_child(&header)?;
            content.append_child(&detail)?;
            item.append_child(&avatar)?;
            item.append_child(&content)?;
            self.participant_list.append_child(&item)?;
        }

        Ok(())
    }

    fn render_browser_access_requests(
        &self,
        requests: &[PendingRequestView],
    ) -> Result<(), JsValue> {
        self.browser_access_list.set_inner_html("");
        for request in requests {
            let item = self.document.create_element("li")?;
            item.set_class_name("browser-access-request");

            let head = self.document.create_element("div")?;
            head.set_class_name("browser-access-request-head");

            let name = self.document.create_element("div")?;
            name.set_class_name("browser-access-request-name");
            name.set_text_content(Some(&request.display_name));

            let time = self.document.create_element("div")?;
            time.set_class_name("browser-access-request-time");
            time.set_text_content(Some(&format_time(request.requested_at)));

            head.append_child(&name)?;
            head.append_child(&time)?;

            let detail = self.document.create_element("div")?;
            detail.set_class_name("browser-access-request-detail");
            let requester = if request.device_label.trim().is_empty() {
                "This browser".to_string()
            } else {
                request.device_label.clone()
            };
            detail.set_text_content(Some(&format!("{requester} wants to join.")));

            let actions = self.document.create_element("div")?;
            actions.set_class_name("browser-access-request-actions");

            let approve = self.document.create_element("button")?;
            approve.set_attribute("type", "button")?;
            approve.set_attribute("data-browser-access-action", "approve")?;
            approve.set_attribute("data-request-id", &request.request_id)?;
            approve.set_text_content(Some("Approve"));

            let deny = self.document.create_element("button")?;
            deny.set_class_name("secondary danger");
            deny.set_attribute("type", "button")?;
            deny.set_attribute("data-browser-access-action", "deny")?;
            deny.set_attribute("data-request-id", &request.request_id)?;
            deny.set_text_content(Some("Deny"));

            actions.append_child(&approve)?;
            actions.append_child(&deny)?;

            item.append_child(&head)?;
            item.append_child(&detail)?;
            item.append_child(&actions)?;
            self.browser_access_list.append_child(&item)?;
        }
        Ok(())
    }

    fn render_room_access(&self, state: &AppState) -> Result<(), JsValue> {
        self.room_policy_list.set_inner_html("");
        self.node_list.set_inner_html("");

        let policy = &state.room_control.access_policy;
        self.append_policy_row(
            "guest",
            "Web guest requests",
            "People joining from the public link still need approval.",
            policy.allow_guest_invites,
        )?;
        self.append_policy_row(
            "member",
            "ElastOS user invites",
            "Invite another trusted ElastOS profile.",
            policy.allow_member_invites,
        )?;
        self.append_policy_row(
            "host",
            "Guest approvals",
            "Trusted ElastOS users may approve web guests.",
            policy.allow_members_to_host_guests,
        )?;

        let guest_sessions = state
            .active_sessions
            .iter()
            .filter(|session| !session.member_bound)
            .collect::<Vec<_>>();
        for session in guest_sessions {
            let item = self.document.create_element("li")?;
            item.set_class_name("node-row");

            let head = self.document.create_element("div")?;
            head.set_class_name("node-row-head");

            let name = self.document.create_element("div")?;
            name.set_class_name("node-row-name");
            name.set_text_content(Some(&session.display_name));

            let detail = self.document.create_element("div")?;
            detail.set_class_name("node-row-detail");
            detail.set_text_content(Some("Web guest"));

            head.append_child(&name)?;
            head.append_child(&detail)?;

            let actions = self.document.create_element("div")?;
            actions.set_class_name("node-row-actions");
            let kick = self.document.create_element("button")?;
            kick.set_class_name("secondary danger");
            kick.set_attribute("type", "button")?;
            kick.set_attribute("data-guest-action", "kick")?;
            kick.set_attribute("data-session-id", &session.session_id)?;
            kick.set_text_content(Some("Kick"));
            actions.append_child(&kick)?;

            item.append_child(&head)?;
            item.append_child(&actions)?;
            self.node_list.append_child(&item)?;
        }

        for member in &state.room_control.members {
            let item = self.document.create_element("li")?;
            item.set_class_name("node-row");

            let head = self.document.create_element("div")?;
            head.set_class_name("node-row-head");

            let name = self.document.create_element("div")?;
            name.set_class_name("node-row-name");
            name.set_text_content(Some(&member_display_name(member)));

            let detail = self.document.create_element("div")?;
            detail.set_class_name("node-row-detail");
            detail.set_text_content(Some(&conversation_role_label(&member.role)));

            head.append_child(&name)?;
            head.append_child(&detail)?;
            item.append_child(&head)?;

            self.node_list.append_child(&item)?;
        }

        for invite in &state.room_control.pending_invites {
            let item = self.document.create_element("li")?;
            item.set_class_name("node-row");

            let head = self.document.create_element("div")?;
            head.set_class_name("node-row-head");

            let name = self.document.create_element("div")?;
            name.set_class_name("node-row-name");
            name.set_text_content(Some("Pending profile invite"));

            let detail = self.document.create_element("div")?;
            detail.set_class_name("node-row-detail");
            detail.set_text_content(Some("Pending ElastOS invite"));

            head.append_child(&name)?;
            head.append_child(&detail)?;

            let actions = self.document.create_element("div")?;
            actions.set_class_name("node-row-actions");
            let cancel = self.document.create_element("button")?;
            cancel.set_class_name("secondary danger");
            cancel.set_attribute("type", "button")?;
            cancel.set_attribute("data-node-action", "revoke-invite")?;
            cancel.set_attribute("data-invite-id", &invite.invite_id)?;
            cancel.set_text_content(Some("Cancel"));
            actions.append_child(&cancel)?;

            item.append_child(&head)?;
            item.append_child(&actions)?;
            self.node_list.append_child(&item)?;
        }

        Ok(())
    }

    fn append_policy_row(
        &self,
        key: &str,
        label: &str,
        detail: &str,
        enabled: bool,
    ) -> Result<(), JsValue> {
        let row = self.document.create_element("div")?;
        row.set_class_name("policy-row");

        let copy = self.document.create_element("div")?;
        let name = self.document.create_element("div")?;
        name.set_class_name("policy-row-name");
        name.set_text_content(Some(label));
        let help = self.document.create_element("div")?;
        help.set_class_name("policy-row-detail");
        help.set_text_content(Some(detail));
        copy.append_child(&name)?;
        copy.append_child(&help)?;

        let toggle = self.document.create_element("button")?;
        toggle.set_attribute("type", "button")?;
        toggle.set_attribute("data-room-policy", key)?;
        toggle.set_attribute("data-enabled", if enabled { "true" } else { "false" })?;
        toggle.set_attribute("aria-pressed", if enabled { "true" } else { "false" })?;
        toggle.set_text_content(Some(if enabled { "On" } else { "Off" }));

        row.append_child(&copy)?;
        row.append_child(&toggle)?;
        self.room_policy_list.append_child(&row)?;
        Ok(())
    }

    fn render_objects(
        &self,
        objects: &[ConversationObjectView],
        attachment_urls: &BTreeMap<String, String>,
    ) -> Result<(), JsValue> {
        self.message_list.set_inner_html("");
        if objects.is_empty() {
            if self.is_shell_mode() && !self.state.borrow().session_active {
                return Ok(());
            }
            let empty = self.document.create_element("li")?;
            empty.set_class_name("empty");
            empty.set_text_content(Some("No messages yet. Say hi."));
            self.message_list.append_child(&empty)?;
            return Ok(());
        }

        for object in objects {
            let item = self.document.create_element("li")?;
            let is_self = object.from_current_session;
            item.set_class_name(
                if is_self && object.kind != ConversationObjectKind::System {
                    "message self-message"
                } else {
                    "message"
                },
            );

            let meta = self.document.create_element("div")?;
            meta.set_class_name("message-meta");
            let sender = self.document.create_element("span")?;
            let sender_name = object_sender_name(object);
            sender.set_text_content(Some(if is_self { "You" } else { sender_name }));
            let time = self.document.create_element("span")?;
            time.set_text_content(Some(&format_time(object.created_at)));
            meta.append_child(&sender)?;
            meta.append_child(&time)?;

            let body = self.document.create_element("div")?;
            match object.kind {
                ConversationObjectKind::System => {
                    item.set_class_name("message system-message");
                    body.set_class_name("message-body system-body");
                    let system_sender = object_sender_name(object);
                    body.set_text_content(Some(
                        &object
                            .body
                            .clone()
                            .map(|body| format!("{system_sender} {body}"))
                            .unwrap_or_else(|| system_sender.to_string()),
                    ));
                }
                ConversationObjectKind::Text => {
                    body.set_class_name("message-body");
                    body.set_text_content(object.body.as_deref());
                }
                ConversationObjectKind::Emoji => {
                    body.set_class_name("message-body emoji-body");
                    body.set_text_content(object.emoji.as_deref());
                }
                ConversationObjectKind::Link => {
                    body.set_class_name("message-body link-body");
                    if let Some(link) = &object.link {
                        let anchor = self.document.create_element("a")?;
                        let is_elastos_uri = link.url.starts_with("elastos://");
                        anchor.set_attribute("href", &link.url)?;
                        if is_elastos_uri {
                            anchor.set_attribute("data-open-uri", &link.url)?;
                            anchor.set_attribute("aria-label", "Open published document")?;
                        } else {
                            anchor.set_attribute("target", "_blank")?;
                            anchor.set_attribute("rel", "noopener noreferrer")?;
                        }

                        let title = self.document.create_element("div")?;
                        title.set_class_name("link-title");
                        title.set_text_content(Some(&link.title));

                        let detail = self.document.create_element("div")?;
                        detail.set_class_name("link-detail");
                        detail.set_text_content(Some(&link.host));

                        anchor.append_child(&title)?;
                        anchor.append_child(&detail)?;
                        body.append_child(&anchor)?;
                    }
                }
                ConversationObjectKind::Attachment => {
                    body.set_class_name("message-body");
                    if let Some(attachment) = &object.attachment {
                        let card = self.document.create_element("div")?;
                        card.set_class_name("attachment-card");

                        if let Some(url) = attachment_urls.get(&attachment.attachment_id) {
                            if attachment.is_image {
                                let image = self.document.create_element("img")?;
                                image.set_class_name("attachment-preview");
                                image.set_attribute("src", url)?;
                                image.set_attribute("alt", &attachment.file_name)?;
                                card.append_child(&image)?;
                            } else if attachment.is_video {
                                let video = self.document.create_element("video")?;
                                video.set_class_name("attachment-preview");
                                video.set_attribute("src", url)?;
                                video.set_attribute("controls", "controls")?;
                                video.set_attribute("preload", "metadata")?;
                                card.append_child(&video)?;
                            } else if attachment.is_audio {
                                let audio = self.document.create_element("audio")?;
                                audio.set_class_name("attachment-audio");
                                audio.set_attribute("src", url)?;
                                audio.set_attribute("controls", "controls")?;
                                audio.set_attribute("preload", "metadata")?;
                                card.append_child(&audio)?;
                            }
                        } else {
                            let loading = self.document.create_element("div")?;
                            loading.set_class_name("attachment-detail");
                            loading.set_text_content(Some("Loading attachment..."));
                            card.append_child(&loading)?;
                        }

                        let name = self.document.create_element("div")?;
                        name.set_class_name("attachment-name");
                        name.set_text_content(Some(&attachment.file_name));

                        let detail = self.document.create_element("div")?;
                        detail.set_class_name("attachment-detail");
                        detail.set_text_content(Some(&format!(
                            "{} · {}",
                            attachment.mime_type,
                            format_bytes(attachment.size_bytes)
                        )));

                        card.append_child(&name)?;
                        card.append_child(&detail)?;
                        if attachment_urls.contains_key(&attachment.attachment_id) {
                            let open = self.document.create_element("button")?;
                            open.set_class_name("attachment-open");
                            open.set_attribute("type", "button")?;
                            open.set_attribute("data-open-attachment", &attachment.attachment_id)?;
                            open.set_text_content(Some("Open in Documents"));
                            card.append_child(&open)?;
                        }
                        body.append_child(&card)?;
                    }
                }
            }

            item.append_child(&meta)?;
            item.append_child(&body)?;
            self.message_list.append_child(&item)?;
        }

        Ok(())
    }
}

fn load_config(document: &Document) -> Result<AppConfig, JsValue> {
    document
        .body()
        .ok_or_else(|| JsValue::from_str("document body unavailable"))?;
    let url = document.url().unwrap_or_default();
    let home_token = extract_fragment_param(&url, "home_token");
    let access_mode = if home_token.is_some() {
        AccessMode::Shell
    } else {
        AccessMode::Gateway
    };
    let initial_join_invite = ["invite", "join", "join_invite"]
        .into_iter()
        .find_map(|key| extract_query_param(&url, key));
    let initial_direct_conversation_id = extract_query_param(&url, "conversation_id");
    Ok(AppConfig {
        access_mode,
        home_token,
        initial_join_invite,
        initial_direct_conversation_id,
        browser_session_request_storage_key: BROWSER_SESSION_REQUEST_STORAGE_KEY.to_string(),
    })
}

fn bump_selection_generation(state: &mut AppState) {
    state.selection_generation = state.selection_generation.wrapping_add(1);
    if state.selection_generation == 0 {
        state.selection_generation = 1;
    }
}

fn current_selection_guard(state: &AppState) -> SelectionGuard {
    SelectionGuard {
        generation: state.selection_generation,
        selected_conversation_id: state.direct.selected_conversation_id.clone(),
    }
}

fn selection_guard_matches(state: &AppState, guard: &SelectionGuard) -> bool {
    state.selection_generation == guard.generation
        && state.direct.selected_conversation_id == guard.selected_conversation_id
}

fn resolve_conversation_choice(
    direct_target_choice: Option<&str>,
    ancestor_choice: Option<&str>,
) -> Option<String> {
    direct_target_choice
        .or(ancestor_choice)
        .map(str::trim)
        .filter(|choice| !choice.is_empty())
        .map(str::to_string)
}

fn conversation_initial(display_name: &str) -> String {
    display_name
        .trim()
        .chars()
        .next()
        .map(|character| character.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_string())
}

fn append_conversation_choice_content(
    document: &Document,
    button: &Element,
    avatar_text: &str,
    title: &str,
    detail: &str,
) -> Result<(), JsValue> {
    let avatar = document.create_element("span")?;
    avatar.set_class_name("conversation-avatar");
    avatar.set_attribute("aria-hidden", "true")?;
    avatar.set_text_content(Some(avatar_text));

    let copy = document.create_element("span")?;
    copy.set_class_name("conversation-choice-copy");
    let name = document.create_element("span")?;
    name.set_class_name("conversation-choice-name");
    name.set_text_content(Some(title));
    let meta = document.create_element("span")?;
    meta.set_class_name("conversation-choice-detail");
    meta.set_text_content(Some(detail));
    copy.append_child(&name)?;
    copy.append_child(&meta)?;

    button.append_child(&avatar)?;
    button.append_child(&copy)?;
    Ok(())
}

fn clear_selected_direct_conversation(state: &mut AppState) {
    state.direct.selected_conversation_id = None;
    state.direct.messages.clear();
    state.direct.pending_send = None;
    state.direct.notice = None;
}

fn commit_shared_selection(state: &mut AppState) -> SelectionGuard {
    bump_selection_generation(state);
    clear_selected_direct_conversation(state);
    current_selection_guard(state)
}

fn commit_direct_selection(state: &mut AppState, conversation_id: &str) -> Option<SelectionGuard> {
    let clean_id = conversation_id.trim();
    if selected_conversation(&state.direct.conversations, clean_id).is_none() {
        return None;
    }
    if state.direct.selected_conversation_id.as_deref() != Some(clean_id) {
        bump_selection_generation(state);
        state.direct.selected_conversation_id = Some(clean_id.to_string());
        state.direct.messages.clear();
        state.direct.pending_send = None;
    }
    state.direct.notice = None;
    Some(current_selection_guard(state))
}

fn commit_requested_direct_selection_if_current(
    state: &mut AppState,
    expected_generation: u64,
    conversation_id: &str,
) -> Option<SelectionGuard> {
    if state.selection_generation != expected_generation {
        return None;
    }
    commit_direct_selection(state, conversation_id)
}

fn apply_active_poll_state(
    state: &mut AppState,
    mut poll: RoomPollView,
) -> (Vec<AttachmentView>, bool) {
    filter_configured_shared_poll(&mut poll);
    let attachments_to_cache = poll
        .objects
        .iter()
        .filter_map(|object| object.attachment.clone())
        .collect::<Vec<_>>();

    let transport_summary = live_status_detail(&poll.transport);
    let was_session_active = state.session_active;
    let previous_display_name = state.display_name.clone();
    let previous_latest_seq = state.latest_seq;
    let previous_participants = state.participants.clone();
    let previous_status_badge = state.status_badge.clone();
    let previous_status_detail = state.status_detail.clone();
    let previous_collaboration_configured = state.collaboration_configured;
    state.collaboration_configured = poll.transport.configured;
    state.session_active = true;
    state.close_leave_sent = false;
    state.display_name = poll.display_name.clone();
    state.status_badge = "Live".to_string();
    state.status_detail = transport_summary;
    state.latest_seq = poll.latest_seq;
    state.objects.extend(poll.objects);
    state.participants = poll.participants;
    dedupe_objects(&mut state.objects);
    if state.collaboration_configured {
        state.objects.retain(configured_shared_object_visible);
        state
            .participants
            .retain(configured_shared_participant_visible);
    }
    if previous_latest_seq != state.latest_seq {
        state.force_message_follow = true;
    }

    let changed = previous_collaboration_configured != state.collaboration_configured
        || was_session_active != state.session_active
        || previous_display_name != state.display_name
        || previous_latest_seq != state.latest_seq
        || previous_participants != state.participants
        || previous_status_badge != state.status_badge
        || previous_status_detail != state.status_detail;
    (attachments_to_cache, changed)
}

fn apply_active_poll_if_current(
    state: &mut AppState,
    guard: &SelectionGuard,
    poll: RoomPollView,
) -> Option<(Vec<AttachmentView>, bool)> {
    selection_guard_matches(state, guard).then(|| apply_active_poll_state(state, poll))
}

fn filter_configured_shared_poll(poll: &mut RoomPollView) {
    if !poll.transport.configured {
        return;
    }
    poll.objects.retain(configured_shared_object_visible);
    poll.participants
        .retain(configured_shared_participant_visible);
}

fn configured_shared_object_visible(object: &ConversationObjectView) -> bool {
    object.sender_profile_verified == Some(true) && !object.sender.trim().is_empty()
}

fn configured_shared_participant_visible(participant: &ParticipantView) -> bool {
    participant.profile_verified == Some(true) && !participant.display_name.trim().is_empty()
}

fn apply_direct_refresh_if_current(
    state: &mut AppState,
    guard: &SelectionGuard,
    conversations: Vec<direct::DirectConversationView>,
    response: DirectMessageList,
) -> Option<Result<bool, u16>> {
    if !selection_guard_matches(state, guard) {
        return None;
    }
    let conversations_changed = state.direct.conversations != conversations;
    state.direct.conversations = conversations;
    Some(
        apply_direct_messages(&mut state.direct, response)
            .map(|messages_changed| conversations_changed || messages_changed)
            .map_err(|_| 500u16),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RenderProjection {
    session_active: bool,
    direct_mode: bool,
    participant_count: String,
}

fn render_projection(state: &AppState, shell_mode: bool) -> RenderProjection {
    let direct_mode = state.direct.selected_conversation_id.is_some();
    let loading_room = shell_mode && !state.session_active && !direct_mode;
    RenderProjection {
        session_active: state.session_active,
        direct_mode,
        participant_count: if loading_room {
            "Opening conversation".to_string()
        } else {
            format_participant_count(state.participants.len())
        },
    }
}

fn apply_summary_state(
    state: &mut AppState,
    summary: &SummaryView,
    access_mode: AccessMode,
) -> bool {
    let previous_room_mode_known = state.room_mode_known;
    let previous_collaboration_configured = state.collaboration_configured;
    let previous_browser_access_allowed = state.browser_access_allowed;
    let previous_browser_access_block_reason = state.browser_access_block_reason.clone();
    let previous_pending_requests = state.pending_requests.clone();
    let previous_active_sessions = state.active_sessions.clone();
    let previous_room_control = state.room_control.clone();
    let previous_status_badge = state.status_badge.clone();
    let previous_status_detail = state.status_detail.clone();
    state.room_mode_known = true;
    state.collaboration_configured = summary.transport.configured;
    if state.collaboration_configured {
        state.browser_access_allowed = summary.browser_access_allowed;
        state.browser_access_block_reason = summary.browser_access_block_reason.clone();
        state.pending_requests.clear();
        state.active_sessions.clear();
        state.room_control = SummaryRoomControlView::default();
        state.show_access_controls = false;
        state.join_invite_url = None;
    } else {
        state.browser_access_allowed = summary.browser_access_allowed;
        state.browser_access_block_reason = summary.browser_access_block_reason.clone();
        state.pending_requests = summary.pending_requests.clone();
        state.active_sessions = summary.active_sessions.clone();
        state.room_control = summary.room_control.clone();
    }
    if !state.session_active && state.request_id.is_none() {
        if access_mode == AccessMode::Shell {
            if state.collaboration_configured
                || summary.local_runtime_role.is_some()
                || summary.browser_access_allowed
            {
                state.status_badge = default_status_badge_for_mode(access_mode);
                state.status_detail = default_status_detail_for_mode(access_mode);
            } else {
                state.status_badge = "Unavailable".to_string();
                let reason = state
                    .browser_access_block_reason
                    .clone()
                    .unwrap_or_else(|| SHELL_ACCESS_UNAVAILABLE_DETAIL.to_string());
                state.status_detail = trim_sentence(&reason);
            }
        } else if !state.browser_access_allowed {
            state.status_badge = "Conversation locked".to_string();
            let reason = state
                .browser_access_block_reason
                .clone()
                .unwrap_or_else(|| "This device is not part of this conversation yet.".to_string());
            state.status_detail = format!(
                "{} Open the conversation from Home first, then try again.",
                trim_sentence(&reason)
            );
        } else {
            state.status_badge = default_status_badge_for_mode(access_mode);
            state.status_detail = default_status_detail_for_mode(access_mode);
        }
    }
    previous_room_mode_known != state.room_mode_known
        || previous_collaboration_configured != state.collaboration_configured
        || previous_browser_access_allowed != state.browser_access_allowed
        || previous_browser_access_block_reason != state.browser_access_block_reason
        || previous_pending_requests != state.pending_requests
        || previous_active_sessions != state.active_sessions
        || previous_room_control != state.room_control
        || previous_status_badge != state.status_badge
        || previous_status_detail != state.status_detail
}

fn apply_summary_if_current(
    state: &mut AppState,
    guard: &SelectionGuard,
    summary: &SummaryView,
    access_mode: AccessMode,
) -> Option<bool> {
    selection_guard_matches(state, guard).then(|| apply_summary_state(state, summary, access_mode))
}

fn load_state(session_storage: Option<&Storage>, config: &AppConfig) -> AppState {
    AppState {
        request_id: if config.access_mode == AccessMode::Gateway {
            session_storage.and_then(|storage| {
                storage
                    .get_item(&config.browser_session_request_storage_key)
                    .ok()
                    .flatten()
            })
        } else {
            None
        },
        pending_chat_send: None,
        selection_generation: 0,
        room_mode_known: false,
        collaboration_configured: false,
        session_active: false,
        close_leave_sent: false,
        poll_loop_started: false,
        show_participants: false,
        show_access_controls: false,
        force_message_follow: true,
        display_name: String::new(),
        status_badge: default_status_badge_for_mode(config.access_mode),
        status_detail: default_status_detail_for_mode(config.access_mode),
        error_transient: false,
        browser_access_allowed: false,
        browser_access_block_reason: None,
        latest_seq: 0,
        objects: Vec::new(),
        participants: Vec::new(),
        pending_requests: Vec::new(),
        active_sessions: Vec::new(),
        room_control: SummaryRoomControlView::default(),
        attachment_urls: BTreeMap::new(),
        join_invite_url: None,
        direct: DirectUiState::default(),
        error_text: None,
    }
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let (_, query_and_fragment) = url.split_once('?')?;
    extract_encoded_param(
        query_and_fragment
            .split_once('#')
            .map_or(query_and_fragment, |(query, _)| query),
        key,
    )
}

fn extract_fragment_param(url: &str, key: &str) -> Option<String> {
    let (_, fragment) = url.split_once('#')?;
    extract_encoded_param(fragment, key)
}

fn extract_encoded_param(encoded: &str, key: &str) -> Option<String> {
    for pair in encoded.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            let decoded = decode_query_value(value);
            if !decoded.trim().is_empty() {
                return Some(decoded);
            }
        }
    }
    None
}

fn decode_query_value(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0usize;
    while index < raw.len() {
        let byte = raw[index];
        if byte == b'+' {
            bytes.push(b' ');
            index += 1;
            continue;
        }
        if byte == b'%' && index + 2 < raw.len() {
            if let (Some(high), Some(low)) = (hex_value(raw[index + 1]), hex_value(raw[index + 2]))
            {
                bytes.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        bytes.push(byte);
        index += 1;
    }
    String::from_utf8(bytes).unwrap_or_else(|_| value.to_string())
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::direct::{
        DirectConversationView, DirectMessageList, DirectUiState, PendingDirectSend,
    };
    use super::{
        apply_active_poll_if_current, apply_active_poll_state, apply_direct_refresh_if_current,
        chat_control_policy, clear_selected_direct_conversation, commit_direct_selection,
        commit_requested_direct_selection_if_current, commit_shared_selection,
        conversation_initial, current_selection_guard, decode_query_value, extract_fragment_param,
        extract_query_param, format_chat_message_request_id, object_sender_name,
        participant_detail, participant_shown_name, pending_chat_request_id, render_projection,
        resolve_conversation_choice, selection_guard_matches, shell_summary_allows_session,
        AccessMode, AppConfig, AppState, ConversationObjectKind, ConversationObjectView,
        ParticipantView, PendingChatSend, RenderProjection, RoomPollView, RoomTransportView,
        ShellSessionBootstrapFailure, ShellSessionStartOutput, SummaryView,
    };

    #[test]
    fn shell_session_bootstrap_errors_are_typed_and_bounded() {
        assert_eq!(
            ShellSessionBootstrapFailure::from_request_error("request failed: 401 secret").detail(),
            "Chat session bootstrap was not authorized. Reopen Chat from Home."
        );
        assert!(
            !ShellSessionBootstrapFailure::from_request_error("provider secret")
                .detail()
                .contains("provider secret")
        );
    }

    #[test]
    fn reads_shell_authority_only_from_the_fragment() {
        let url = "http://localhost/apps/chat-room/?invite=peer&conversation_id=direct%3Aone#home_token=scope%2D123";
        assert_eq!(
            extract_fragment_param(url, "home_token").as_deref(),
            Some("scope-123")
        );
        assert_eq!(extract_query_param(url, "home_token"), None);
        assert_eq!(
            extract_query_param(url, "conversation_id").as_deref(),
            Some("direct:one")
        );
    }

    #[test]
    fn decodes_query_value_for_invite_urls() {
        assert_eq!(
            decode_query_value("elastos%3A%2F%2Fpeer%2Finvite%3Ftoken%3Dabc-123"),
            "elastos://peer/invite?token=abc-123"
        );
    }

    #[test]
    fn preserves_invalid_percent_escapes() {
        assert_eq!(decode_query_value("abc%ZZ%2"), "abc%ZZ%2");
    }

    #[test]
    fn configured_chat_is_home_only_and_hides_every_gateway_control() {
        let summary: SummaryView = serde_json::from_value(serde_json::json!({
            "room_slug": "chat-room",
            "pending_count": 0,
            "active_session_count": 0,
            "browser_access_allowed": false,
            "transport": {
                "configured": true,
                "available": true
            },
        }))
        .unwrap();
        assert!(shell_summary_allows_session(&summary));
        assert!(!summary.browser_access_allowed);

        let policy = chat_control_policy(true, true, true, true, true, true);
        assert!(policy.enable_text_send);
        assert!(!policy.show_attach);
        assert!(!policy.enable_attach);
        assert!(!policy.show_browser_requests);
        assert!(!policy.show_room_access_toggle);
        assert!(!policy.show_room_access);
        assert!(!policy.show_conversation_join);
        assert!(!policy.enable_gateway_controls);

        let public_policy = chat_control_policy(true, true, false, false, true, true);
        assert!(!public_policy.enable_text_send);
        assert!(!public_policy.show_browser_requests);
        assert!(!public_policy.show_conversation_join);
    }

    #[test]
    fn configured_chat_uses_verified_profile_names_without_device_details() {
        let participant = ParticipantView {
            display_name: "Owner".to_string(),
            profile_verified: Some(true),
            device_label: "MacBook".to_string(),
            last_seen_at: 0,
            role: Some("owner".to_string()),
            local_session_count: 1,
            is_current_session: false,
        };
        assert_eq!(participant_shown_name(&participant), "Owner");
        assert_eq!(participant_detail(&participant, false, false), "active now");

        let object = ConversationObjectView {
            seq: 1,
            sender: "Owner".to_string(),
            sender_profile_verified: Some(true),
            from_current_session: false,
            kind: ConversationObjectKind::Text,
            body: Some("hello".to_string()),
            emoji: None,
            link: None,
            attachment: None,
            created_at: 1,
        };
        assert_eq!(object_sender_name(&object), "Owner");
    }

    #[test]
    fn configured_chat_defensively_omits_false_or_none_rows_before_render() {
        let mut state = AppState::default();
        let poll = RoomPollView {
            room_slug: "chat-room".to_string(),
            display_name: "Shared room".to_string(),
            latest_seq: 4,
            participants: vec![
                ParticipantView {
                    display_name: "Owner".to_string(),
                    profile_verified: Some(true),
                    device_label: String::new(),
                    last_seen_at: 1,
                    role: Some("owner".to_string()),
                    local_session_count: 1,
                    is_current_session: true,
                },
                ParticipantView {
                    display_name: "Wrong endpoint".to_string(),
                    profile_verified: Some(false),
                    device_label: "Laptop".to_string(),
                    last_seen_at: 1,
                    role: Some("member".to_string()),
                    local_session_count: 1,
                    is_current_session: false,
                },
                ParticipantView {
                    display_name: "Unsigned guest".to_string(),
                    profile_verified: None,
                    device_label: "Browser".to_string(),
                    last_seen_at: 1,
                    role: None,
                    local_session_count: 1,
                    is_current_session: false,
                },
                ParticipantView {
                    display_name: String::new(),
                    profile_verified: Some(true),
                    device_label: String::new(),
                    last_seen_at: 1,
                    role: Some("member".to_string()),
                    local_session_count: 1,
                    is_current_session: false,
                },
            ],
            objects: vec![
                ConversationObjectView {
                    seq: 1,
                    sender: "Owner".to_string(),
                    sender_profile_verified: Some(true),
                    from_current_session: true,
                    kind: ConversationObjectKind::Text,
                    body: Some("hello".to_string()),
                    emoji: None,
                    link: None,
                    attachment: None,
                    created_at: 1,
                },
                ConversationObjectView {
                    seq: 2,
                    sender: "Wrong profile".to_string(),
                    sender_profile_verified: Some(false),
                    from_current_session: false,
                    kind: ConversationObjectKind::Text,
                    body: Some("bad-profile".to_string()),
                    emoji: None,
                    link: None,
                    attachment: None,
                    created_at: 2,
                },
                ConversationObjectView {
                    seq: 3,
                    sender: "Wrong endpoint".to_string(),
                    sender_profile_verified: Some(false),
                    from_current_session: false,
                    kind: ConversationObjectKind::Text,
                    body: Some("bad-endpoint".to_string()),
                    emoji: None,
                    link: None,
                    attachment: None,
                    created_at: 3,
                },
                ConversationObjectView {
                    seq: 4,
                    sender: String::new(),
                    sender_profile_verified: None,
                    from_current_session: false,
                    kind: ConversationObjectKind::Text,
                    body: Some("unsigned".to_string()),
                    emoji: None,
                    link: None,
                    attachment: None,
                    created_at: 4,
                },
            ],
            transport: RoomTransportView {
                configured: true,
                available: true,
                status: None,
            },
        };

        // This is display defense only: configured shared Chat refuses to
        // render rows not already marked verified by the server projection.
        let (_attachments, changed) = apply_active_poll_state(&mut state, poll);
        assert!(changed);
        assert!(state.collaboration_configured);
        assert_eq!(state.participants.len(), 1);
        assert_eq!(state.participants[0].display_name, "Owner");
        assert_eq!(state.participants[0].profile_verified, Some(true));
        assert_eq!(state.objects.len(), 1);
        assert_eq!(state.objects[0].body.as_deref(), Some("hello"));
        assert_eq!(object_sender_name(&state.objects[0]), "Owner");
    }

    #[test]
    fn browser_access_defaults_fail_closed_without_exposing_gateway_controls_in_shell_mode() {
        let summary: SummaryView = serde_json::from_value(serde_json::json!({
            "room_slug": "chat-room",
            "pending_count": 0,
            "active_session_count": 0,
        }))
        .unwrap();
        assert!(!summary.browser_access_allowed);
        assert!(summary.room_control.access_policy.allow_guest_invites);
        assert!(summary.room_control.access_policy.allow_member_invites);
        assert!(
            summary
                .room_control
                .access_policy
                .allow_members_to_host_guests
        );

        let shell_isolated = chat_control_policy(true, false, true, true, true, true);
        assert!(shell_isolated.enable_text_send);
        assert!(shell_isolated.show_attach);
        assert!(shell_isolated.enable_attach);
        assert!(!shell_isolated.show_browser_requests);
        assert!(!shell_isolated.show_room_access_toggle);
        assert!(!shell_isolated.show_room_access);
        assert!(!shell_isolated.show_conversation_join);
        assert!(!shell_isolated.enable_gateway_controls);

        let gateway_isolated = chat_control_policy(true, false, false, true, true, true);
        assert!(gateway_isolated.show_browser_requests);
        assert!(gateway_isolated.show_room_access_toggle);
        assert!(gateway_isolated.show_room_access);
        assert!(!gateway_isolated.show_conversation_join);
        assert!(gateway_isolated.enable_gateway_controls);
    }

    #[test]
    fn chat_controls_require_an_explicitly_known_room_mode() {
        let unknown = chat_control_policy(false, false, true, false, true, true);
        assert!(!unknown.show_attach);
        assert!(!unknown.enable_attach);
        assert!(!unknown.show_browser_requests);
        assert!(!unknown.show_room_access_toggle);
        assert!(!unknown.show_room_access);
        assert!(!unknown.show_conversation_join);
        assert!(!unknown.enable_gateway_controls);

        let configured = chat_control_policy(true, true, true, false, true, true);
        assert!(!configured.show_conversation_join);

        let unconfigured_gateway = chat_control_policy(true, false, false, false, false, false);
        assert!(unconfigured_gateway.show_conversation_join);

        let unconfigured_shell = chat_control_policy(true, false, true, false, false, false);
        assert!(!unconfigured_shell.show_conversation_join);
    }

    #[test]
    fn chat_request_id_is_stable_for_failed_retry_and_changes_only_with_intent() {
        let mut pending = None;
        let first = pending_chat_request_id(&mut pending, "hello", || {
            Ok("chat-message:00000000000000000000000000000001".to_string())
        })
        .unwrap();
        assert_eq!(
            pending_chat_request_id(&mut pending, "hello", || {
                panic!("same-body retry must not generate another request ID")
            })
            .unwrap(),
            first
        );

        let changed = pending_chat_request_id(&mut pending, "changed", || {
            Ok("chat-message:00000000000000000000000000000002".to_string())
        })
        .unwrap();
        assert_ne!(changed, first);
        assert_eq!(
            pending,
            Some(PendingChatSend {
                request_id: changed,
                body: "changed".to_string(),
            })
        );
    }

    #[test]
    fn chat_request_id_has_one_fixed_bounded_lowercase_hex_shape() {
        let request_id = format_chat_message_request_id([0xab; 16]);
        assert_eq!(request_id, "chat-message:abababababababababababababababab");
        assert_eq!(request_id.len(), 45);
    }

    #[test]
    fn shared_selection_clears_direct_state_and_valid_shell_start_activates_shared_projection() {
        let config = AppConfig {
            access_mode: AccessMode::Shell,
            home_token: Some("test-token".to_string()),
            initial_join_invite: None,
            initial_direct_conversation_id: None,
            browser_session_request_storage_key: "test-key".to_string(),
        };
        let direct_messages: DirectMessageList = serde_json::from_value(serde_json::json!({
            "conversation_id": "direct:sha256:fixture-conversation",
            "messages": [{
                "message_id": "message:fixture-direct",
                "direction": "incoming",
                "text": "hello from direct",
                "created_at": 1_725_000_000u64,
                "delivery_state": "received"
            }]
        }))
        .unwrap();
        let mut state = AppState {
            session_active: false,
            direct: DirectUiState {
                selected_conversation_id: Some("direct:sha256:fixture-conversation".to_string()),
                messages: direct_messages.messages,
                pending_send: Some(PendingDirectSend {
                    request_id: "chat-message:fixture".to_string(),
                    conversation_id: "direct:sha256:fixture-conversation".to_string(),
                    text: "hello".to_string(),
                }),
                notice: Some("temporary".to_string()),
                ..DirectUiState::default()
            },
            ..super::load_state(None, &config)
        };

        clear_selected_direct_conversation(&mut state);
        assert!(state.direct.selected_conversation_id.is_none());
        assert!(state.direct.messages.is_empty());
        assert!(state.direct.pending_send.is_none());
        assert!(state.direct.notice.is_none());

        let start: ShellSessionStartOutput = serde_json::from_value(serde_json::json!({
            "poll": {
                "room_slug": "chat-room",
                "display_name": "Configured User",
                "latest_seq": 0,
                "participants": [{
                    "display_name": "Configured User",
                    "device_label": "ElastOS shell",
                    "last_seen_at": 1,
                    "role": null,
                    "local_session_count": 1,
                    "is_current_session": true
                }],
                "objects": [],
                "transport": {
                    "configured": true,
                    "available": true,
                    "status": "Collaboration is configured."
                }
            }
        }))
        .unwrap();

        let (attachments, changed) = apply_active_poll_state(&mut state, start.poll);
        assert!(attachments.is_empty());
        assert!(changed);
        assert!(state.session_active);

        assert_eq!(
            render_projection(&state, config.access_mode == AccessMode::Shell),
            RenderProjection {
                session_active: true,
                direct_mode: false,
                participant_count: super::format_participant_count(1),
            }
        );
    }

    #[test]
    fn direct_to_shared_commits_shared_loading_synchronously() {
        let config = AppConfig {
            access_mode: AccessMode::Shell,
            home_token: Some("test-token".to_string()),
            initial_join_invite: None,
            initial_direct_conversation_id: None,
            browser_session_request_storage_key: "test-key".to_string(),
        };
        let mut state = AppState {
            direct: DirectUiState {
                selected_conversation_id: Some("direct:sha256:fixture-conversation".to_string()),
                messages: vec![serde_json::from_value(serde_json::json!({
                    "message_id": "message:fixture-direct",
                    "direction": "incoming",
                    "text": "hello from direct",
                    "created_at": 1_725_000_000u64,
                    "delivery_state": "received"
                }))
                .unwrap()],
                pending_send: Some(PendingDirectSend {
                    request_id: "chat-message:fixture".to_string(),
                    conversation_id: "direct:sha256:fixture-conversation".to_string(),
                    text: "hello".to_string(),
                }),
                notice: Some("temporary".to_string()),
                ..DirectUiState::default()
            },
            ..super::load_state(None, &config)
        };
        let guard = commit_shared_selection(&mut state);
        assert_eq!(guard.selected_conversation_id, None);
        assert!(selection_guard_matches(&state, &guard));
        assert_eq!(state.selection_generation, 1);
        assert_eq!(
            render_projection(&state, true),
            RenderProjection {
                session_active: false,
                direct_mode: false,
                participant_count: "Opening conversation".to_string(),
            }
        );
    }

    #[test]
    fn stale_requested_direct_selection_after_shared_choice_is_ignored() {
        let config = AppConfig {
            access_mode: AccessMode::Shell,
            home_token: Some("test-token".to_string()),
            initial_join_invite: None,
            initial_direct_conversation_id: Some("direct:sha256:fixture-conversation".to_string()),
            browser_session_request_storage_key: "test-key".to_string(),
        };
        let mut state = super::load_state(None, &config);
        state.direct.conversations = vec![DirectConversationView {
            conversation_id: "direct:sha256:fixture-conversation".to_string(),
            display_name: "Fixture Friend".to_string(),
            removed: false,
        }];
        let bootstrap_generation = state.selection_generation;
        let shared_guard = commit_shared_selection(&mut state);
        let stale = commit_requested_direct_selection_if_current(
            &mut state,
            bootstrap_generation,
            "direct:sha256:fixture-conversation",
        );
        assert!(stale.is_none());
        assert!(selection_guard_matches(&state, &shared_guard));
        assert!(state.direct.selected_conversation_id.is_none());
    }

    #[test]
    fn stale_direct_completion_after_shared_selection_is_ignored() {
        let config = AppConfig {
            access_mode: AccessMode::Shell,
            home_token: Some("test-token".to_string()),
            initial_join_invite: None,
            initial_direct_conversation_id: None,
            browser_session_request_storage_key: "test-key".to_string(),
        };
        let mut state = super::load_state(None, &config);
        state.direct.conversations = vec![DirectConversationView {
            conversation_id: "direct:sha256:fixture-conversation".to_string(),
            display_name: "Fixture Friend".to_string(),
            removed: false,
        }];
        let direct_guard =
            commit_direct_selection(&mut state, "direct:sha256:fixture-conversation")
                .expect("direct selection should be available");
        let shared_guard = commit_shared_selection(&mut state);
        let direct_messages: DirectMessageList = serde_json::from_value(serde_json::json!({
            "conversation_id": "direct:sha256:fixture-conversation",
            "messages": [{
                "message_id": "message:fixture-direct",
                "direction": "incoming",
                "text": "hello from direct",
                "created_at": 1_725_000_000u64,
                "delivery_state": "received"
            }]
        }))
        .unwrap();
        let stale = apply_direct_refresh_if_current(
            &mut state,
            &direct_guard,
            vec![DirectConversationView {
                conversation_id: "direct:sha256:fixture-conversation".to_string(),
                display_name: "Fixture Friend".to_string(),
                removed: false,
            }],
            direct_messages,
        );
        assert!(stale.is_none());
        assert!(selection_guard_matches(&state, &shared_guard));
        assert!(state.direct.selected_conversation_id.is_none());
    }

    #[test]
    fn stale_shared_completion_after_later_direct_selection_is_ignored() {
        let config = AppConfig {
            access_mode: AccessMode::Shell,
            home_token: Some("test-token".to_string()),
            initial_join_invite: None,
            initial_direct_conversation_id: None,
            browser_session_request_storage_key: "test-key".to_string(),
        };
        let mut state = super::load_state(None, &config);
        state.direct.conversations = vec![DirectConversationView {
            conversation_id: "direct:sha256:fixture-conversation".to_string(),
            display_name: "Fixture Friend".to_string(),
            removed: false,
        }];
        let shared_guard = current_selection_guard(&state);
        let direct_guard =
            commit_direct_selection(&mut state, "direct:sha256:fixture-conversation")
                .expect("direct selection should be available");
        let poll: super::RoomPollView = serde_json::from_value(serde_json::json!({
            "room_slug": "chat-room",
            "display_name": "Configured User",
            "latest_seq": 0,
            "participants": [{
                "display_name": "Configured User",
                "device_label": "ElastOS shell",
                "last_seen_at": 1,
                "role": null,
                "local_session_count": 1,
                "is_current_session": true
            }],
            "objects": [],
            "transport": {
                "configured": true,
                "available": true,
                "status": "Collaboration is configured."
            }
        }))
        .unwrap();
        let stale = apply_active_poll_if_current(&mut state, &shared_guard, poll);
        assert!(stale.is_none());
        assert!(selection_guard_matches(&state, &direct_guard));
        assert_eq!(
            state.direct.selected_conversation_id.as_deref(),
            Some("direct:sha256:fixture-conversation")
        );
        assert!(!state.session_active);
    }

    #[test]
    fn text_node_clicks_resolve_to_the_same_choice_intent() {
        assert_eq!(
            resolve_conversation_choice(None, Some("shared")).as_deref(),
            Some("shared")
        );
        assert_eq!(
            resolve_conversation_choice(Some("direct:sha256:fixture-conversation"), None)
                .as_deref(),
            Some("direct:sha256:fixture-conversation")
        );
        assert_eq!(resolve_conversation_choice(None, None), None);
    }

    #[test]
    fn conversation_initial_uses_the_first_visible_character() {
        assert_eq!(conversation_initial("  alice"), "A");
        assert_eq!(conversation_initial("Élodie"), "É");
        assert_eq!(conversation_initial("   "), "?");
    }
}

fn default_status_badge_for_mode(access_mode: AccessMode) -> String {
    match access_mode {
        AccessMode::Shell => String::new(),
        AccessMode::Gateway => "Join".to_string(),
    }
}

fn default_status_detail_for_mode(access_mode: AccessMode) -> String {
    match access_mode {
        AccessMode::Shell => String::new(),
        AccessMode::Gateway => "Enter your name to ask to join this conversation.".to_string(),
    }
}

fn dedupe_objects(objects: &mut Vec<ConversationObjectView>) {
    objects.sort_by_key(|object| object.seq);
    objects.dedup_by_key(|object| object.seq);
}

fn should_follow_scroll(element: &HtmlElement) -> bool {
    let gap = element.scroll_height() - element.client_height() - element.scroll_top();
    gap <= AUTO_SCROLL_THRESHOLD_PX
}

fn restore_scroll_position(element: &HtmlElement, scroll_top: i32) {
    element.set_scroll_top(scroll_top);
}

fn scroll_to_bottom(element: &HtmlElement) {
    element.set_scroll_top(element.scroll_height());
}

fn schedule_scroll_to_bottom(element: HtmlElement) {
    let raf_element = element.clone();
    let timeout_element = element.clone();
    let late_element = element.clone();
    let callback = Closure::<dyn FnMut()>::wrap(Box::new(move || {
        raf_element.set_scroll_top(raf_element.scroll_height());
    }));
    if let Some(window) = window() {
        let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
    }
    callback.forget();

    let timeout = Closure::<dyn FnMut()>::wrap(Box::new(move || {
        timeout_element.set_scroll_top(timeout_element.scroll_height());
    }));
    if let Some(window) = window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            timeout.as_ref().unchecked_ref(),
            80,
        );
    }
    timeout.forget();

    let late_timeout = Closure::<dyn FnMut()>::wrap(Box::new(move || {
        late_element.set_scroll_top(late_element.scroll_height());
    }));
    if let Some(window) = window() {
        let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(
            late_timeout.as_ref().unchecked_ref(),
            220,
        );
    }
    late_timeout.forget();
}

fn new_chat_message_request_id() -> Result<String, String> {
    let crypto = window()
        .ok_or_else(|| "window unavailable".to_string())?
        .crypto()
        .map_err(js_error)?;
    let mut random = [0u8; 16];
    crypto
        .get_random_values_with_u8_array(&mut random)
        .map_err(js_error)?;
    Ok(format_chat_message_request_id(random))
}

fn format_chat_message_request_id(random: [u8; 16]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";

    let mut request_id = String::with_capacity("chat-message:".len() + 32);
    request_id.push_str("chat-message:");
    for byte in random {
        request_id.push(HEX[usize::from(byte >> 4)] as char);
        request_id.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    request_id
}

fn pending_chat_request_id<F>(
    pending: &mut Option<PendingChatSend>,
    body: &str,
    generate: F,
) -> Result<String, String>
where
    F: FnOnce() -> Result<String, String>,
{
    if let Some(existing) = pending.as_ref().filter(|existing| existing.body == body) {
        return Ok(existing.request_id.clone());
    }
    let request_id = generate()?;
    *pending = Some(PendingChatSend {
        request_id: request_id.clone(),
        body: body.to_string(),
    });
    Ok(request_id)
}

fn is_session_error(err: &str) -> bool {
    err.contains("invalid or expired session") || err.contains("request failed: 401")
}

fn shell_summary_allows_session(summary: &SummaryView) -> bool {
    summary.transport.configured
        || summary.local_runtime_role.is_some()
        || summary.browser_access_allowed
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChatControlPolicy {
    enable_text_send: bool,
    show_attach: bool,
    enable_attach: bool,
    show_browser_requests: bool,
    show_room_access_toggle: bool,
    show_room_access: bool,
    show_conversation_join: bool,
    enable_gateway_controls: bool,
}

fn chat_control_policy(
    room_mode_known: bool,
    configured: bool,
    shell_mode: bool,
    session_active: bool,
    show_access_controls: bool,
    has_pending_requests: bool,
) -> ChatControlPolicy {
    let gateway_surface = room_mode_known && !shell_mode && !configured;
    ChatControlPolicy {
        enable_text_send: session_active,
        show_attach: room_mode_known && !configured,
        enable_attach: room_mode_known && !configured && session_active,
        show_browser_requests: gateway_surface && has_pending_requests,
        show_room_access_toggle: gateway_surface && session_active,
        show_room_access: gateway_surface && session_active && show_access_controls,
        show_conversation_join: gateway_surface && !session_active,
        enable_gateway_controls: gateway_surface && session_active,
    }
}

fn live_status_detail(transport: &RoomTransportView) -> String {
    if !transport.available {
        return transport
            .status
            .clone()
            .unwrap_or_else(|| "Collaboration is isolated on this Runtime.".to_string());
    }
    transport
        .status
        .clone()
        .unwrap_or_else(|| "Collaboration is configured.".to_string())
}

fn format_participant_count(count: usize) -> String {
    format!("People · {count}")
}

fn format_browser_access_count(count: usize) -> String {
    match count {
        1 => "Join Requests · 1".to_string(),
        _ => format!("Join Requests · {count}"),
    }
}

fn format_participant_toggle_label(count: usize, open: bool) -> String {
    let noun = match count {
        1 => "1 person".to_string(),
        _ => format!("{count} people"),
    };
    if open {
        format!("Hide people · {noun}")
    } else {
        format!("People · {noun}")
    }
}

fn participant_role_badge(role: &str) -> String {
    match role {
        "owner" | "admin" | "member" => "ElastOS".to_string(),
        _ => title_case(role),
    }
}

fn conversation_role_label(role: &str) -> String {
    match role {
        "owner" | "admin" => "Conversation manager".to_string(),
        "member" => "Trusted participant".to_string(),
        _ => title_case(role),
    }
}

fn member_display_name(member: &RoomMemberView) -> String {
    member
        .profile_card
        .as_ref()
        .map(|profile| profile.display_name.trim())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| UNVERIFIED_ROOM_MEMBER_NAME.to_string())
}

fn participant_detail(participant: &ParticipantView, is_local: bool, shell_mode: bool) -> String {
    let mut parts = Vec::new();
    let local_runtime_participant =
        participant.is_current_session && participant.device_label.trim() == "ElastOS shell";
    let verified_profile_identity = participant.profile_verified == Some(true);
    let generic_browser_label = matches!(
        participant.device_label.trim(),
        "" | "Browser" | "This browser"
    );
    if !participant.device_label.trim().is_empty()
        && !verified_profile_identity
        && !local_runtime_participant
        && !generic_browser_label
    {
        parts.push(participant.device_label.clone());
    }
    if is_local || local_runtime_participant {
        parts.push("active now".to_string());
    } else if participant.local_session_count > 1 {
        parts.push(format!("{} browsers", participant.local_session_count));
    } else if participant.role.is_some() {
        parts.push("active now".to_string());
    } else if participant.local_session_count > 0 {
        parts.push("web guest".to_string());
    }
    if participant.last_seen_at > 0 && !is_local && !local_runtime_participant && !shell_mode {
        parts.push(format!("seen {}", format_time(participant.last_seen_at)));
    }
    parts.join(" · ")
}

/// The declared direct-conversation attachment policy, stated where the
/// person meets it. Attachments need object handling and a delivery path of
/// their own, designed on the unified delivery layer — until then the attach
/// control in direct mode is visibly unavailable, never silently missing.
const DIRECT_ATTACHMENTS_UNAVAILABLE: &str = "Direct conversations are text-only for now.";

/// Unconfigured room-control membership may still lack a profile card; keep
/// that legacy surface unchanged. Configured shared Chat never renders this
/// marker because it hides room controls and filters person rows earlier.
const UNVERIFIED_ROOM_MEMBER_NAME: &str = "Unverified member";

fn object_sender_name(object: &ConversationObjectView) -> &str {
    &object.sender
}

fn participant_shown_name(participant: &ParticipantView) -> &str {
    &participant.display_name
}

fn participant_initial(name: &str) -> String {
    name.chars()
        .find(|ch| ch.is_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

fn trim_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.ends_with('.') {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

fn room_access_policy_enabled_default() -> bool {
    true
}

fn format_time(timestamp_secs: u64) -> String {
    let iso = js_sys::Date::new(&(timestamp_secs as f64 * 1000.0).into())
        .to_iso_string()
        .as_string()
        .unwrap_or_default();
    if iso.len() >= 16 {
        iso[11..16].to_string()
    } else {
        timestamp_secs.to_string()
    }
}

fn format_bytes(size_bytes: u64) -> String {
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
        format!("{} bytes", size_bytes)
    }
}

fn set_hidden(element: &HtmlElement, hidden: bool) -> Result<(), JsValue> {
    if hidden {
        element.set_attribute("hidden", "")
    } else {
        element.remove_attribute("hidden")
    }
}

async fn copy_text_to_clipboard(text: &str) -> Result<(), String> {
    let window = window().ok_or_else(|| "window unavailable".to_string())?;
    let write_text = Reflect::get(window.as_ref(), &JsValue::from_str("elastosChatCopyInvite"))
        .and_then(|value| value.dyn_into::<Function>())
        .map_err(|_| "Trusted Home Clipboard is unavailable.".to_string())?;
    let promise = write_text
        .call1(&JsValue::UNDEFINED, &JsValue::from_str(text))
        .map_err(js_error)?;
    JsFuture::from(Promise::from(promise))
        .await
        .map_err(js_error)?;
    Ok(())
}

async fn api_post_json<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
) -> Result<TResp, String> {
    api_post_json_with_headers(path, body, &[]).await
}

async fn direct_get_json<T: DeserializeOwned>(
    path: &str,
    extra_headers: &[(&str, String)],
) -> Result<T, u16> {
    let mut request = Request::get(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|_| 0u16)?;
    if response.status() != 200 {
        return Err(response.status());
    }
    response.json::<T>().await.map_err(|_| 500)
}

async fn direct_post_json<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
    extra_headers: &[(&str, String)],
) -> Result<(u16, TResp), u16> {
    let mut request = Request::post(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request
        .json(body)
        .map_err(|_| 400u16)?
        .send()
        .await
        .map_err(|_| 0u16)?;
    let status = response.status();
    if status != 200 && status != 202 {
        return Err(response.status());
    }
    response
        .json::<TResp>()
        .await
        .map(|body| (status, body))
        .map_err(|_| 500)
}

async fn api_post_session_json<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
    extra_headers: &[(&str, String)],
) -> Result<TResp, String> {
    let mut request = Request::post(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request
        .json(body)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response
        .json::<TResp>()
        .await
        .map_err(|err| err.to_string())
}

async fn api_get_session_bytes(
    path: &str,
    extra_headers: &[(&str, String)],
) -> Result<Vec<u8>, String> {
    let mut request = Request::get(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response.binary().await.map_err(|err| err.to_string())
}

async fn api_post_session_bytes<TResp: DeserializeOwned>(
    path: &str,
    bytes: &[u8],
    extra_headers: &[(&str, String)],
) -> Result<TResp, String> {
    let mut request = Request::post(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    request = request.header("content-type", "application/octet-stream");
    let body = Uint8Array::new_with_length(bytes.len() as u32);
    body.copy_from(bytes);
    let response = request
        .body(body)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response
        .json::<TResp>()
        .await
        .map_err(|err| err.to_string())
}

async fn api_get_json_with_headers<T: DeserializeOwned>(
    path: &str,
    extra_headers: &[(&str, String)],
) -> Result<T, String> {
    let mut request = Request::get(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response.json::<T>().await.map_err(|err| err.to_string())
}

async fn api_post_json_with_headers<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
    extra_headers: &[(&str, String)],
) -> Result<TResp, String> {
    let mut request = Request::post(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request
        .json(body)
        .map_err(|err| err.to_string())?
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response
        .json::<TResp>()
        .await
        .map_err(|err| err.to_string())
}

async fn api_post_empty_json_with_headers<TResp: DeserializeOwned>(
    path: &str,
    extra_headers: &[(&str, String)],
) -> Result<TResp, String> {
    let mut request = Request::post(path).credentials(RequestCredentials::SameOrigin);
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request.send().await.map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response
        .json::<TResp>()
        .await
        .map_err(|err| err.to_string())
}

fn send_keepalive_post(path: &str, extra_headers: &[(&str, String)]) -> bool {
    let Some(window) = window() else {
        return false;
    };
    let init = JsObject::new();
    if Reflect::set(
        &init,
        &JsValue::from_str("method"),
        &JsValue::from_str("POST"),
    )
    .is_err()
    {
        return false;
    }
    if Reflect::set(
        &init,
        &JsValue::from_str("credentials"),
        &JsValue::from_str("same-origin"),
    )
    .is_err()
    {
        return false;
    }
    if Reflect::set(&init, &JsValue::from_str("keepalive"), &JsValue::TRUE).is_err() {
        return false;
    }
    if !extra_headers.is_empty() {
        let headers = JsObject::new();
        for (name, value) in extra_headers {
            if Reflect::set(
                &headers,
                &JsValue::from_str(name),
                &JsValue::from_str(value),
            )
            .is_err()
            {
                return false;
            }
        }
        if Reflect::set(&init, &JsValue::from_str("headers"), &headers).is_err() {
            return false;
        }
    }
    let Ok(fetch) = Reflect::get(window.as_ref(), &JsValue::from_str("fetch"))
        .and_then(|value| value.dyn_into::<Function>())
    else {
        return false;
    };
    fetch
        .call2(window.as_ref(), &JsValue::from_str(path), init.as_ref())
        .is_ok()
}

fn element_by_id(document: &Document, id: &str) -> Result<HtmlElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing element #{id}")))?
        .dyn_into::<HtmlElement>()?)
}

fn optional_element_by_id(document: &Document, id: &str) -> Option<HtmlElement> {
    document
        .get_element_by_id(id)
        .and_then(|element| element.dyn_into::<HtmlElement>().ok())
}

fn form_by_id(document: &Document, id: &str) -> Result<HtmlFormElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing form #{id}")))?
        .dyn_into::<HtmlFormElement>()?)
}

fn input_by_id(document: &Document, id: &str) -> Result<HtmlInputElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing input #{id}")))?
        .dyn_into::<HtmlInputElement>()?)
}

fn textarea_by_id(document: &Document, id: &str) -> Result<HtmlTextAreaElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing textarea #{id}")))?
        .dyn_into::<HtmlTextAreaElement>()?)
}

fn button_by_id(document: &Document, id: &str) -> Result<HtmlButtonElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing button #{id}")))?
        .dyn_into::<HtmlButtonElement>()?)
}

fn js_string_field(data: &JsValue, key: &str) -> Option<String> {
    Reflect::get(data, &JsValue::from_str(key))
        .ok()
        .and_then(|value| value.as_string())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn js_blob_field(data: &JsValue, key: &str) -> Option<Blob> {
    Reflect::get(data, &JsValue::from_str(key))
        .ok()
        .filter(|value| !value.is_null() && !value.is_undefined())
        .and_then(|value| value.dyn_into::<Blob>().ok())
}

async fn blob_to_bytes(blob: Blob) -> Result<Vec<u8>, String> {
    let buffer = JsFuture::from(blob.array_buffer())
        .await
        .map_err(js_error)?;
    let array = Uint8Array::new(&buffer);
    let mut bytes = vec![0; array.length() as usize];
    array.copy_to(&mut bytes);
    Ok(bytes)
}

fn runtime_event_is_chat_room(event: &MessageEvent) -> bool {
    let Some(window) = window() else {
        return false;
    };
    let Ok(origin) = window.location().origin() else {
        return false;
    };
    if event.origin() != origin {
        return false;
    }
    let data = event.data();
    if js_string_field(&data, "type").as_deref() != Some("elastos:runtime-events") {
        return false;
    }
    let Ok(events) = Reflect::get(&data, &JsValue::from_str("events")) else {
        return false;
    };
    if !Array::is_array(&events) {
        return false;
    }
    let events = Array::from(&events);
    for event in events.iter() {
        let scope = js_string_field(&event, "scope").unwrap_or_default();
        let kind = js_string_field(&event, "kind").unwrap_or_default();
        if scope == "chat-room" || kind.starts_with("chat-room.") {
            return true;
        }
    }
    false
}

fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "browser storage error".to_string())
}
