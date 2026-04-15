use base64::Engine as _;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use gloo_net::http::Request;
use gloo_timers::future::TimeoutFuture;
use js_sys::Uint8Array;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    window, Document, Event, HtmlButtonElement, HtmlElement, HtmlFormElement, HtmlInputElement,
    RequestCredentials, Storage,
};

const SESSION_REQUEST_ID: &str = "room_browser.request_id";
const APP_ID: &str = "room-browser";
const BROWSER_SESSION_API_BASE: &str = "/api/browser/session";
const ROOM_API_BASE: &str = "/api/apps/room-browser";
const POLL_INTERVAL_MS: u32 = 1200;
const AUTO_SCROLL_THRESHOLD_PX: i32 = 48;
const EMOJI_BUTTON_IDS: [(&str, &str); 6] = [
    ("emoji-wave", "👋"),
    ("emoji-fire", "🔥"),
    ("emoji-party", "🎉"),
    ("emoji-clap", "👏"),
    ("emoji-heart", "❤️"),
    ("emoji-eyes", "👀"),
];

#[derive(Debug, Clone, Default)]
struct AppState {
    request_id: Option<String>,
    paired: bool,
    show_participants: bool,
    force_message_follow: bool,
    display_name: String,
    device_label: String,
    local_runtime_did: Option<String>,
    status_badge: String,
    status_detail: String,
    error_text: Option<String>,
    session_text: String,
    pairing_allowed: bool,
    pairing_block_reason: Option<String>,
    latest_seq: u64,
    objects: Vec<ConversationObjectView>,
    participants: Vec<ParticipantView>,
    attachment_urls: BTreeMap<String, String>,
}

struct App {
    state: RefCell<AppState>,
    document: Document,
    body: HtmlElement,
    session_storage: Storage,
    status_badge: HtmlElement,
    status_detail: HtmlElement,
    error_text: HtmlElement,
    pair_card: HtmlElement,
    pair_form: HtmlFormElement,
    display_name_input: HtmlInputElement,
    device_label_input: HtmlInputElement,
    pair_submit: HtmlButtonElement,
    reset_button: HtmlButtonElement,
    chat_card: HtmlElement,
    session_text: HtmlElement,
    message_list: HtmlElement,
    composer_form: HtmlFormElement,
    message_input: HtmlInputElement,
    attachment_input: HtmlInputElement,
    attach_button: HtmlButtonElement,
    send_button: HtmlButtonElement,
    leave_button: HtmlButtonElement,
    participant_toggle: HtmlButtonElement,
    participant_close: HtmlButtonElement,
    participant_scrim: HtmlElement,
    emoji_buttons: Vec<HtmlButtonElement>,
    participant_count: HtmlElement,
    participant_list: HtmlElement,
}

#[derive(Debug, Clone, Deserialize)]
struct PairRequestOutput {
    request_id: String,
}

#[derive(Debug, Clone, Deserialize)]
struct PairStatusOutput {
    status: String,
    expires_at: Option<u64>,
    denial_reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct ConversationObjectView {
    seq: u64,
    sender: String,
    #[serde(default)]
    sender_member_did: Option<String>,
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
    device_label: String,
    last_seen_at: u64,
    #[serde(default)]
    member_did: Option<String>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    local_session_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomPollView {
    #[allow(dead_code)]
    room_slug: String,
    display_name: String,
    expires_at: u64,
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
    local_runtime_did: Option<String>,
    #[serde(default)]
    local_runtime_role: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    ephemeral_hosted_guest_url: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    room_control: SummaryRoomControlView,
    #[serde(default = "summary_pairing_allowed_default")]
    pairing_allowed: bool,
    #[serde(default)]
    pairing_block_reason: Option<String>,
    #[serde(default)]
    transport: RoomTransportView,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
struct RoomTransportView {
    #[allow(dead_code)]
    available: bool,
    #[allow(dead_code)]
    connected_peer_count: usize,
    #[allow(dead_code)]
    topic: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct SummaryRoomControlView {
    #[serde(default)]
    #[allow(dead_code)]
    access_policy: RoomAccessPolicyView,
}

#[derive(Debug, Clone, Deserialize)]
struct RoomAccessPolicyView {
    #[serde(default = "summary_pairing_allowed_default")]
    #[allow(dead_code)]
    allow_guest_invites: bool,
    #[serde(default = "summary_pairing_allowed_default")]
    #[allow(dead_code)]
    allow_member_invites: bool,
    #[serde(default = "summary_pairing_allowed_default")]
    #[allow(dead_code)]
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

#[derive(Debug, Serialize)]
struct PairRequestInput<'a> {
    app: &'a str,
    display_name: &'a str,
    device_label: &'a str,
}

#[derive(Debug, Serialize)]
struct PairStatusInput<'a> {
    app: &'a str,
    request_id: &'a str,
}

#[derive(Debug, Serialize)]
struct RoomPollInput {
    since: u64,
}

#[derive(Debug, Serialize)]
struct SendMessageInput<'a> {
    body: &'a str,
}

#[derive(Debug, Serialize)]
struct UploadAttachmentStartInput<'a> {
    file_name: &'a str,
    mime_type: &'a str,
    size_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct UploadAttachmentStartOutput {
    upload_id: String,
    chunk_size_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct UploadAttachmentChunkOutput {
    #[allow(dead_code)]
    upload_id: String,
    #[allow(dead_code)]
    received_bytes: u64,
    #[allow(dead_code)]
    size_bytes: u64,
    #[allow(dead_code)]
    complete: bool,
}

#[wasm_bindgen(start)]
pub fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();

    let window = window().ok_or_else(|| JsValue::from_str("window unavailable"))?;
    let document = window
        .document()
        .ok_or_else(|| JsValue::from_str("document unavailable"))?;
    let session_storage = window
        .session_storage()?
        .ok_or_else(|| JsValue::from_str("sessionStorage unavailable"))?;

    let app = Rc::new(App {
        state: RefCell::new(load_state(&session_storage)),
        document: document.clone(),
        body: document
            .body()
            .ok_or_else(|| JsValue::from_str("document body unavailable"))?,
        session_storage,
        status_badge: element_by_id(&document, "status-badge")?,
        status_detail: element_by_id(&document, "status-detail")?,
        error_text: element_by_id(&document, "error-text")?,
        pair_card: element_by_id(&document, "pair-card")?,
        pair_form: form_by_id(&document, "pair-form")?,
        display_name_input: input_by_id(&document, "display-name")?,
        device_label_input: input_by_id(&document, "device-label")?,
        pair_submit: button_by_id(&document, "pair-submit")?,
        reset_button: button_by_id(&document, "reset-button")?,
        chat_card: element_by_id(&document, "chat-card")?,
        session_text: element_by_id(&document, "session-text")?,
        message_list: element_by_id(&document, "message-list")?,
        composer_form: form_by_id(&document, "composer-form")?,
        message_input: input_by_id(&document, "message-input")?,
        attachment_input: input_by_id(&document, "attachment-input")?,
        attach_button: button_by_id(&document, "attach-button")?,
        send_button: button_by_id(&document, "send-button")?,
        leave_button: button_by_id(&document, "leave-button")?,
        participant_toggle: button_by_id(&document, "participant-toggle")?,
        participant_close: button_by_id(&document, "participant-close")?,
        participant_scrim: element_by_id(&document, "participant-scrim")?,
        emoji_buttons: EMOJI_BUTTON_IDS
            .iter()
            .map(|(id, _)| button_by_id(&document, id))
            .collect::<Result<Vec<_>, _>>()?,
        participant_count: element_by_id(&document, "participant-count")?,
        participant_list: element_by_id(&document, "participant-list")?,
    });

    {
        let state = app.state.borrow();
        app.display_name_input.set_value(&state.display_name);
        app.device_label_input.set_value(&state.device_label);
    }

    app.bind_events()?;
    app.hydrate_defaults();
    app.render()?;
    app.restore_session();

    let poll_app = Rc::clone(&app);
    spawn_local(async move {
        loop {
            let had_error = poll_app.state.borrow().error_text.is_some();
            match poll_app.poll_once().await {
                Ok(changed) => {
                    poll_app.clear_error();
                    if changed || had_error {
                        let _ = poll_app.render();
                    }
                }
                Err(err) => {
                    poll_app.set_error(Some(err));
                    let _ = poll_app.render();
                }
            }
            TimeoutFuture::new(POLL_INTERVAL_MS).await;
        }
    });

    Ok(())
}

impl App {
    fn apply_summary(&self, summary: &SummaryView) {
        let mut state = self.state.borrow_mut();
        state.local_runtime_did = summary.local_runtime_did.clone();
        state.pairing_allowed = summary.pairing_allowed;
        state.pairing_block_reason = summary.pairing_block_reason.clone();
        if !state.paired && state.request_id.is_none() {
            if !state.pairing_allowed {
                state.status_badge = "Room locked".to_string();
                let reason = state
                    .pairing_block_reason
                    .clone()
                    .unwrap_or_else(|| "This runtime is not in this room yet.".to_string());
                state.status_detail = format!(
                    "{} Join or import the room on this device first, then reopen this page.",
                    trim_sentence(&reason)
                );
            } else {
                let access_hint = if summary.local_runtime_role.is_some() {
                    "Name this browser, request pairing, then approve it from room controls."
                } else {
                    "Name this browser, request pairing, then ask a nearby room member to approve it from room controls."
                };
                let transport = summary
                    .transport
                    .status
                    .as_deref()
                    .map(trim_sentence)
                    .unwrap_or_else(|| "Local room runtime ready.".to_string());
                state.status_badge = "Ready".to_string();
                state.status_detail = format!("{access_hint} {transport}");
            }
        }
    }

    fn bind_events(self: &Rc<Self>) -> Result<(), JsValue> {
        let pair_app = Rc::clone(self);
        let pair_submit = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |event: Event| {
            event.prevent_default();
            let app = Rc::clone(&pair_app);
            spawn_local(async move {
                app.clear_error();
                if let Err(err) = app.submit_pair_request().await {
                    app.set_error(Some(err));
                }
                let _ = app.render();
            });
        }));
        self.pair_form
            .add_event_listener_with_callback("submit", pair_submit.as_ref().unchecked_ref())?;
        pair_submit.forget();

        let reset_app = Rc::clone(self);
        let reset_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            reset_app.reset_pairing();
            let _ = reset_app.render();
        }));
        self.reset_button
            .add_event_listener_with_callback("click", reset_click.as_ref().unchecked_ref())?;
        reset_click.forget();

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

        let attach_trigger_app = Rc::clone(self);
        let attach_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            let _ = attach_trigger_app.attachment_input.click();
        }));
        self.attach_button
            .add_event_listener_with_callback("click", attach_click.as_ref().unchecked_ref())?;
        attach_click.forget();

        let upload_app = Rc::clone(self);
        let upload_change = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            let app = Rc::clone(&upload_app);
            spawn_local(async move {
                app.clear_error();
                if let Err(err) = app.upload_selected_attachment().await {
                    app.set_error(Some(err));
                }
                let _ = app.render();
            });
        }));
        self.attachment_input
            .add_event_listener_with_callback("change", upload_change.as_ref().unchecked_ref())?;
        upload_change.forget();

        let leave_app = Rc::clone(self);
        let leave_click = Closure::<dyn FnMut(Event)>::wrap(Box::new(move |_event: Event| {
            let app = Rc::clone(&leave_app);
            spawn_local(async move {
                app.clear_error();
                if let Err(err) = app.leave_room().await {
                    app.set_error(Some(err));
                }
                let _ = app.render();
            });
        }));
        self.leave_button
            .add_event_listener_with_callback("click", leave_click.as_ref().unchecked_ref())?;
        leave_click.forget();

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

        Ok(())
    }

    fn clear_error(&self) {
        self.state.borrow_mut().error_text = None;
    }

    fn set_error(&self, error: Option<String>) {
        self.state.borrow_mut().error_text = error;
    }

    fn restore_session(self: &Rc<Self>) {
        let should_restore = {
            let state = self.state.borrow();
            state.request_id.is_none() && !state.paired
        };
        if !should_restore {
            return;
        }

        let app = Rc::clone(self);
        spawn_local(async move {
            match api_post_session_json(
                &format!("{ROOM_API_BASE}/poll"),
                &RoomPollInput { since: 0 },
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
                    app.set_error(Some(err));
                    let _ = app.render();
                }
            }
        });
    }

    fn apply_active_poll(&self, poll: RoomPollView) -> (Vec<AttachmentView>, bool) {
        let attachments_to_cache = poll
            .objects
            .iter()
            .filter_map(|object| object.attachment.clone())
            .collect::<Vec<_>>();

        let transport_summary = room_transport_summary(&poll.transport);
        {
            let mut state = self.state.borrow_mut();
            let was_paired = state.paired;
            let previous_display_name = state.display_name.clone();
            let previous_latest_seq = state.latest_seq;
            let previous_participants = state.participants.clone();
            let previous_status_badge = state.status_badge.clone();
            let previous_status_detail = state.status_detail.clone();
            let previous_session_text = state.session_text.clone();
            state.paired = true;
            state.display_name = poll.display_name.clone();
            state.status_badge = "Live".to_string();
            state.status_detail = transport_summary.clone();
            state.session_text = format!(
                "Paired as {}. Session expires {}.",
                poll.display_name,
                format_time(poll.expires_at),
            );
            state.latest_seq = poll.latest_seq;
            state.objects.extend(poll.objects);
            state.participants = poll.participants;
            dedupe_objects(&mut state.objects);
            if previous_latest_seq != state.latest_seq {
                state.force_message_follow = true;
            }

            let changed = was_paired != state.paired
                || previous_display_name != state.display_name
                || previous_latest_seq != state.latest_seq
                || previous_participants != state.participants
                || previous_status_badge != state.status_badge
                || previous_status_detail != state.status_detail
                || previous_session_text != state.session_text;
            return (attachments_to_cache, changed);
        }
    }

    fn reset_pairing(&self) {
        self.session_storage.remove_item(SESSION_REQUEST_ID).ok();
        let mut state = self.state.borrow_mut();
        state.request_id = None;
        state.paired = false;
        state.show_participants = false;
        state.force_message_follow = true;
        state.latest_seq = 0;
        state.objects.clear();
        state.participants.clear();
        state.attachment_urls.clear();
        state.status_badge = "Ready".to_string();
        state.status_detail = "Name this browser and approve it from room controls.".to_string();
        state.session_text = "Session inactive.".to_string();
        state.error_text = None;
    }

    fn handle_session_loss(&self, detail: &str) {
        self.session_storage.remove_item(SESSION_REQUEST_ID).ok();
        let mut state = self.state.borrow_mut();
        state.request_id = None;
        state.paired = false;
        state.show_participants = false;
        state.force_message_follow = true;
        state.latest_seq = 0;
        state.objects.clear();
        state.participants.clear();
        state.attachment_urls.clear();
        state.status_badge = "Pair again".to_string();
        state.status_detail = detail.to_string();
        state.session_text = "Session inactive.".to_string();
        state.error_text = None;
    }

    async fn leave_room(&self) -> Result<(), String> {
        if !self.state.borrow().paired {
            return Ok(());
        }
        let _: ConversationObjectView =
            api_post_session_empty_json(&format!("{ROOM_API_BASE}/session/leave")).await?;
        let mut state = self.state.borrow_mut();
        state.paired = false;
        state.show_participants = false;
        state.force_message_follow = true;
        state.latest_seq = 0;
        state.objects.clear();
        state.participants.clear();
        state.attachment_urls.clear();
        state.status_badge = "Pair again".to_string();
        state.status_detail = "Session cleared on this browser. Pair again when ready.".to_string();
        state.session_text = "Session inactive.".to_string();
        Ok(())
    }

    fn hydrate_defaults(self: &Rc<Self>) {
        let should_fetch = {
            let state = self.state.borrow();
            state.display_name.trim().is_empty() && state.request_id.is_none() && !state.paired
        };
        if !should_fetch {
            return;
        }

        let app = Rc::clone(self);
        spawn_local(async move {
            let Ok(summary) =
                api_get_json::<SummaryView>(&format!("{ROOM_API_BASE}/summary")).await
            else {
                return;
            };
            app.apply_summary(&summary);
            let _ = app.render();
        });
    }

    async fn submit_pair_request(&self) -> Result<(), String> {
        let already_paired_or_pending = {
            let state = self.state.borrow();
            state.request_id.is_some() || state.paired
        };
        if already_paired_or_pending {
            return Ok(());
        }

        let mut display_name = self.display_name_input.value().trim().to_string();
        let summary = api_get_json::<SummaryView>(&format!("{ROOM_API_BASE}/summary"))
            .await
            .ok();
        if let Some(summary) = &summary {
            self.apply_summary(summary);
            if !summary.pairing_allowed {
                return Err(summary
                    .pairing_block_reason
                    .clone()
                    .unwrap_or_else(|| "This runtime cannot pair into this room.".to_string()));
            }
        }
        if display_name.is_empty() {
            display_name = {
                let state = self.state.borrow();
                state.display_name.trim().to_string()
            };
        }
        if display_name.is_empty() {
            return Err("Display name is empty. Set a name before requesting pairing.".to_string());
        }

        let device_label = self.device_label_input.value().trim().to_string();
        let payload = PairRequestInput {
            app: APP_ID,
            display_name: &display_name,
            device_label: &device_label,
        };
        let response: PairRequestOutput =
            api_post_json(&format!("{BROWSER_SESSION_API_BASE}/pair"), &payload).await?;

        self.session_storage
            .set_item(SESSION_REQUEST_ID, &response.request_id)
            .map_err(js_error)?;

        let mut state = self.state.borrow_mut();
        state.request_id = Some(response.request_id);
        state.display_name = display_name;
        state.device_label = device_label;
        state.status_badge = "Pending approval".to_string();
        state.status_detail =
            "A room member needs to approve this browser before it can join the room.".to_string();
        Ok(())
    }

    async fn send_message(&self) -> Result<(), String> {
        if !self.state.borrow().paired {
            return Ok(());
        }
        let body = self.message_input.value().trim().to_string();
        if body.is_empty() {
            return Ok(());
        }

        let payload = SendMessageInput { body: &body };
        let sent: ConversationObjectView = match api_post_session_json(
            &format!("{ROOM_API_BASE}/objects/send"),
            &payload,
        )
        .await
        {
            Ok(sent) => sent,
            Err(err) if is_session_error(&err) => {
                self.handle_session_loss(
                    "This browser session was ended by room controls. Request approval again to re-enter.",
                );
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        self.message_input.set_value("");

        let mut state = self.state.borrow_mut();
        state.latest_seq = sent.seq;
        state.objects.push(sent);
        state.force_message_follow = true;
        Ok(())
    }

    async fn upload_selected_attachment(&self) -> Result<(), String> {
        if !self.state.borrow().paired {
            return Ok(());
        }

        let file = self
            .attachment_input
            .files()
            .and_then(|files| files.get(0))
            .ok_or_else(|| "No file selected.".to_string())?;
        let file_name = file.name();
        let mime_type = file.type_();
        let buffer = JsFuture::from(file.array_buffer())
            .await
            .map_err(js_error)?;
        let bytes = Uint8Array::new(&buffer).to_vec();
        let start: UploadAttachmentStartOutput = api_post_session_json(
            &format!("{ROOM_API_BASE}/upload/start"),
            &UploadAttachmentStartInput {
                file_name: &file_name,
                mime_type: &mime_type,
                size_bytes: bytes.len() as u64,
            },
        )
        .await?;

        for (index, chunk) in bytes.chunks(start.chunk_size_bytes).enumerate() {
            let _: UploadAttachmentChunkOutput = api_post_session_bytes_json(
                &format!("{ROOM_API_BASE}/upload/{}/chunk", start.upload_id),
                chunk,
                &[(
                    "x-elastos-upload-offset",
                    (index * start.chunk_size_bytes).to_string(),
                )],
            )
            .await?;
        }

        let sent: ConversationObjectView = match api_post_session_empty_json(&format!(
            "{ROOM_API_BASE}/upload/{}/finish",
            start.upload_id
        ))
        .await
        {
            Ok(sent) => sent,
            Err(err) if is_session_error(&err) => {
                self.handle_session_loss(
                    "This browser session was ended by room controls. Request approval again to re-enter.",
                );
                return Ok(());
            }
            Err(err) => return Err(err),
        };
        self.attachment_input.set_value("");

        let attachment_to_cache = sent.attachment.clone();
        {
            let mut state = self.state.borrow_mut();
            state.latest_seq = sent.seq;
            state.objects.push(sent);
            state.force_message_follow = true;
        }
        if let Some(attachment) = attachment_to_cache {
            self.cache_attachment_data_url(&attachment).await?;
        }
        Ok(())
    }

    async fn poll_once(&self) -> Result<bool, String> {
        let paired = {
            let state = self.state.borrow();
            state.paired
        };
        if paired {
            let poll: RoomPollView = match api_post_session_json(
                &format!("{ROOM_API_BASE}/poll"),
                &RoomPollInput {
                    since: {
                        let state = self.state.borrow();
                        state.latest_seq
                    },
                },
            )
            .await
            {
                Ok(poll) => poll,
                Err(err) if is_session_error(&err) => {
                    self.handle_session_loss(
                        "This browser session was ended by room controls. Request approval again to re-enter.",
                    );
                    return Ok(true);
                }
                Err(err) => return Err(err),
            };

            let (attachments_to_cache, changed) = self.apply_active_poll(poll);
            for attachment in attachments_to_cache {
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

        let status: PairStatusOutput = api_post_json(
            &format!("{BROWSER_SESSION_API_BASE}/status"),
            &PairStatusInput {
                app: APP_ID,
                request_id: &request_id,
            },
        )
        .await?;

        match status.status.as_str() {
            "pending" => {
                let mut state = self.state.borrow_mut();
                let changed = state.status_badge != "Waiting"
                    || state.status_detail != "Waiting for room approval. Keep this page open.";
                state.status_badge = "Waiting".to_string();
                state.status_detail = "Waiting for room approval. Keep this page open.".to_string();
                return Ok(changed);
            }
            "approved" => {
                self.session_storage.remove_item(SESSION_REQUEST_ID).ok();

                let mut state = self.state.borrow_mut();
                let previous_badge = state.status_badge.clone();
                let previous_detail = state.status_detail.clone();
                state.request_id = None;
                state.paired = true;
                state.status_badge = "Joining".to_string();
                if let Some(expires_at) = status.expires_at {
                    state.status_detail = format!(
                        "Approved in room controls. Joining room now. Session expires {}.",
                        format_time(expires_at)
                    );
                } else {
                    state.status_detail =
                        "Approved in room controls. Joining room now.".to_string();
                }
                return Ok(
                    previous_badge != state.status_badge || previous_detail != state.status_detail
                );
            }
            "denied" => {
                self.session_storage.remove_item(SESSION_REQUEST_ID).ok();
                let mut state = self.state.borrow_mut();
                state.request_id = None;
                state.status_badge = "Denied".to_string();
                state.status_detail = status
                    .denial_reason
                    .clone()
                    .unwrap_or_else(|| "A room member denied this pairing request.".to_string());
                return Ok(true);
            }
            "expired" => {
                self.session_storage.remove_item(SESSION_REQUEST_ID).ok();
                let mut state = self.state.borrow_mut();
                state.request_id = None;
                state.status_badge = "Expired".to_string();
                state.status_detail =
                    "This pairing request expired. Submit a new one when ready.".to_string();
                return Ok(true);
            }
            other => {
                let mut state = self.state.borrow_mut();
                state.status_badge = "Unknown".to_string();
                state.status_detail = format!("Unexpected pairing status: {}.", other);
                return Ok(true);
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

        let bytes = match api_get_session_bytes(&format!(
            "{ROOM_API_BASE}/attachments/{}",
            attachment.attachment_id
        ))
        .await
        {
            Ok(bytes) => bytes,
            Err(err) if is_session_error(&err) => {
                self.handle_session_loss(
                    "This browser session was ended by room controls. Request approval again to re-enter.",
                );
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

        self.status_badge
            .set_text_content(Some(&state.status_badge));
        self.status_detail
            .set_text_content(Some(&state.status_detail));
        self.session_text
            .set_text_content(Some(&state.session_text));
        self.participant_count
            .set_text_content(Some(&format_participant_count(state.participants.len())));
        self.participant_toggle
            .set_text_content(Some(&format_participant_toggle_label(
                state.participants.len(),
                state.show_participants,
            )));
        let pending = state.request_id.is_some();
        let paired = state.paired;
        self.participant_toggle.set_attribute(
            "aria-expanded",
            if state.show_participants {
                "true"
            } else {
                "false"
            },
        )?;
        self.body
            .set_attribute("data-room-paired", if paired { "true" } else { "false" })?;
        self.chat_card.set_attribute(
            "data-roster-open",
            if state.show_participants {
                "true"
            } else {
                "false"
            },
        )?;
        set_hidden(&self.participant_scrim, !state.show_participants)?;
        self.participant_scrim.set_attribute(
            "aria-hidden",
            if state.show_participants {
                "false"
            } else {
                "true"
            },
        )?;

        if let Some(error) = &state.error_text {
            self.error_text.remove_attribute("hidden")?;
            self.error_text.set_text_content(Some(error));
        } else {
            self.error_text.set_attribute("hidden", "")?;
            self.error_text.set_text_content(None);
        }

        set_hidden(&self.pair_card, paired)?;
        set_hidden(&self.chat_card, !paired)?;
        set_hidden(&self.reset_button, !(pending || !paired))?;

        self.display_name_input
            .set_disabled(pending || !state.pairing_allowed);
        self.device_label_input
            .set_disabled(pending || !state.pairing_allowed);
        self.pair_submit
            .set_disabled(pending || !state.pairing_allowed);
        self.message_input.set_disabled(!paired);
        self.attachment_input.set_disabled(!paired);
        self.attach_button.set_disabled(!paired);
        self.send_button.set_disabled(!paired);
        for button in &self.emoji_buttons {
            button.set_disabled(!paired);
        }

        self.render_participants(&state.participants)?;
        self.render_objects(&state.objects, &state.attachment_urls)?;
        restore_scroll_position(&self.participant_list, previous_participant_scroll_top);
        if force_message_follow || follow_messages {
            scroll_to_bottom(&self.message_list);
            schedule_scroll_to_bottom(self.message_list.clone());
        } else {
            restore_scroll_position(&self.message_list, previous_message_scroll_top);
        }
        Ok(())
    }

    fn render_participants(&self, participants: &[ParticipantView]) -> Result<(), JsValue> {
        self.participant_list.set_inner_html("");
        if participants.is_empty() {
            let empty = self.document.create_element("li")?;
            empty.set_class_name("empty");
            empty.set_text_content(Some("Waiting for another runtime."));
            self.participant_list.append_child(&empty)?;
            return Ok(());
        }

        let state = self.state.borrow().clone();
        for participant in participants {
            let item = self.document.create_element("li")?;
            let is_local = participant
                .member_did
                .as_deref()
                .zip(state.local_runtime_did.as_deref())
                .map(|(left, right)| left == right)
                .unwrap_or_else(|| participant.display_name == state.display_name);
            item.set_class_name(if is_local {
                "participant participant-local"
            } else {
                "participant"
            });

            let avatar = self.document.create_element("div")?;
            avatar.set_class_name("participant-avatar");
            avatar.set_text_content(Some(&participant_initial(&participant.display_name)));

            let content = self.document.create_element("div")?;
            content.set_class_name("participant-content");

            let header = self.document.create_element("div")?;
            header.set_class_name("participant-header");

            let name = self.document.create_element("div")?;
            name.set_class_name("participant-name");
            name.set_text_content(Some(&participant.display_name));
            header.append_child(&name)?;

            let badge_text = if is_local {
                Some("You".to_string())
            } else if let Some(role) = participant.role.as_deref() {
                Some(title_case(role))
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
            detail.set_text_content(Some(&participant_detail(participant)));

            content.append_child(&header)?;
            content.append_child(&detail)?;
            item.append_child(&avatar)?;
            item.append_child(&content)?;
            self.participant_list.append_child(&item)?;
        }

        Ok(())
    }

    fn render_objects(
        &self,
        objects: &[ConversationObjectView],
        attachment_urls: &BTreeMap<String, String>,
    ) -> Result<(), JsValue> {
        self.message_list.set_inner_html("");
        if objects.is_empty() {
            let empty = self.document.create_element("li")?;
            empty.set_class_name("empty");
            empty.set_text_content(Some("No messages yet. Say hi."));
            self.message_list.append_child(&empty)?;
            return Ok(());
        }

        let state = self.state.borrow().clone();
        for object in objects {
            let item = self.document.create_element("li")?;
            let is_self = object
                .sender_member_did
                .as_deref()
                .zip(state.local_runtime_did.as_deref())
                .map(|(left, right)| left == right)
                .unwrap_or_else(|| object.sender == state.display_name);
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
            sender.set_text_content(Some(if is_self { "You" } else { &object.sender }));
            let time = self.document.create_element("span")?;
            time.set_text_content(Some(&format_time(object.created_at)));
            meta.append_child(&sender)?;
            meta.append_child(&time)?;

            let body = self.document.create_element("div")?;
            match object.kind {
                ConversationObjectKind::System => {
                    item.set_class_name("message system-message");
                    body.set_class_name("message-body system-body");
                    body.set_text_content(Some(
                        &object
                            .body
                            .clone()
                            .map(|body| format!("{} {}", object.sender, body))
                            .unwrap_or_else(|| object.sender.clone()),
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
                        anchor.set_attribute("href", &link.url)?;
                        anchor.set_attribute("target", "_blank")?;
                        anchor.set_attribute("rel", "noopener noreferrer")?;

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
                        if let Some(url) = attachment_urls.get(&attachment.attachment_id) {
                            let open = self.document.create_element("a")?;
                            open.set_class_name("attachment-open");
                            open.set_attribute("href", url)?;
                            open.set_attribute("target", "_blank")?;
                            open.set_attribute("rel", "noopener noreferrer")?;
                            open.set_text_content(Some("Open"));
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

fn load_state(session_storage: &Storage) -> AppState {
    AppState {
        request_id: session_storage.get_item(SESSION_REQUEST_ID).ok().flatten(),
        paired: false,
        show_participants: false,
        force_message_follow: true,
        display_name: String::new(),
        device_label: String::new(),
        local_runtime_did: None,
        status_badge: "Ready".to_string(),
        status_detail: "Name this browser and approve it from room controls.".to_string(),
        session_text: "Session inactive.".to_string(),
        pairing_allowed: true,
        pairing_block_reason: None,
        latest_seq: 0,
        objects: Vec::new(),
        participants: Vec::new(),
        attachment_urls: BTreeMap::new(),
        error_text: None,
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

fn is_session_error(err: &str) -> bool {
    err.contains("invalid or expired session") || err.contains("request failed: 401")
}

fn room_transport_summary(transport: &RoomTransportView) -> String {
    if transport.connected_peer_count > 0 {
        return format!(
            "Connected to {} runtime{}.",
            transport.connected_peer_count,
            if transport.connected_peer_count == 1 {
                ""
            } else {
                "s"
            }
        );
    }
    transport
        .status
        .as_deref()
        .map(trim_sentence)
        .unwrap_or_else(|| "Waiting for another room runtime.".to_string())
}

fn format_participant_count(count: usize) -> String {
    match count {
        0 => "No one in room".to_string(),
        1 => "1 in room".to_string(),
        _ => format!("{count} in room"),
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

fn participant_detail(participant: &ParticipantView) -> String {
    let mut parts = Vec::new();
    if !participant.device_label.trim().is_empty() {
        parts.push(participant.device_label.clone());
    }
    if participant.local_session_count > 1 {
        parts.push(format!("{} browsers", participant.local_session_count));
    } else if participant.role.is_some() {
        parts.push("active now".to_string());
    } else if participant.local_session_count > 0 {
        parts.push("guest browser".to_string());
    }
    if participant.last_seen_at > 0 {
        parts.push(format!("seen {}", format_time(participant.last_seen_at)));
    }
    parts.join(" · ")
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

fn summary_pairing_allowed_default() -> bool {
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

async fn api_get_json<T: DeserializeOwned>(path: &str) -> Result<T, String> {
    let response = Request::get(path)
        .credentials(RequestCredentials::SameOrigin)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response.json::<T>().await.map_err(|err| err.to_string())
}

async fn api_post_json<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
) -> Result<TResp, String> {
    let response = Request::post(path)
        .credentials(RequestCredentials::SameOrigin)
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

async fn api_post_session_json<TReq: Serialize, TResp: DeserializeOwned>(
    path: &str,
    body: &TReq,
) -> Result<TResp, String> {
    let response = Request::post(path)
        .credentials(RequestCredentials::SameOrigin)
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

async fn api_post_session_empty_json<TResp: DeserializeOwned>(path: &str) -> Result<TResp, String> {
    let response = Request::post(path)
        .credentials(RequestCredentials::SameOrigin)
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

async fn api_post_session_bytes_json<TResp: DeserializeOwned>(
    path: &str,
    bytes: &[u8],
    extra_headers: &[(&str, String)],
) -> Result<TResp, String> {
    let mut request = Request::post(path)
        .credentials(RequestCredentials::SameOrigin)
        .header("Content-Type", "application/octet-stream");
    for (name, value) in extra_headers {
        request = request.header(name, value);
    }
    let response = request
        .body(bytes.to_vec())
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

async fn api_get_session_bytes(path: &str) -> Result<Vec<u8>, String> {
    let response = Request::get(path)
        .credentials(RequestCredentials::SameOrigin)
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.ok() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!("request failed: {} {}", response.status(), body));
    }
    response.binary().await.map_err(|err| err.to_string())
}

fn element_by_id(document: &Document, id: &str) -> Result<HtmlElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing element #{id}")))?
        .dyn_into::<HtmlElement>()?)
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

fn button_by_id(document: &Document, id: &str) -> Result<HtmlButtonElement, JsValue> {
    Ok(document
        .get_element_by_id(id)
        .ok_or_else(|| JsValue::from_str(&format!("missing button #{id}")))?
        .dyn_into::<HtmlButtonElement>()?)
}

fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "browser storage error".to_string())
}
