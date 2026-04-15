use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use elastos_common::localhost::rooted_localhost_fs_path;
use serde::{Deserialize, Serialize};

use crate::room_service::RoomSummary;

const NOTIFICATIONS_SCHEMA: &str = "elastos.notifications/v1";
const NOTIFICATIONS_ROOT_URI: &str = "localhost://Local/Shared/System/Notifications";
const NOTIFICATIONS_FILE: &str = "notifications.json";
const NOTIFICATION_EVENTS_FILE: &str = "events.json";
const NATIVE_CHAT_RELAY_STATE_FILE: &str = "native-chat-relay.json";
const NATIVE_CHAT_ROOT_URI: &str = "localhost://Users/self/.AppData/LocalHost/Chat";
const NATIVE_CHAT_RELAY_CHANNEL: &str = "!pc2";
const ROOM_PAIR_REQUEST_KIND: &str = "room_pair_request";
const ROOM_PAIRING_TTL_SECS: u64 = 10 * 60;
const NOTIFICATION_EVENTS_SCHEMA: &str = "elastos.notification-events/v1";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NotificationSummary {
    #[serde(default)]
    pub unread_count: usize,
    #[serde(default)]
    pub attention_count: usize,
    #[serde(default)]
    pub entries: Vec<NotificationEntryView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEntryView {
    pub id: String,
    pub source_app: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub action_ref: Option<NotificationActionRef>,
    pub created_at: u64,
    #[serde(default)]
    pub expires_at: Option<u64>,
    pub severity: NotificationSeverity,
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub acted: bool,
    #[serde(default)]
    pub dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationActionRef {
    pub app: String,
    pub action_id: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationSeverity {
    Info,
    Attention,
    Critical,
}

impl Default for NotificationSeverity {
    fn default() -> Self {
        Self::Info
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotificationStore {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    entries: Vec<NotificationEntryRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationEntryRecord {
    id: String,
    source_app: String,
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    action_ref: Option<NotificationActionRef>,
    created_at: u64,
    #[serde(default)]
    expires_at: Option<u64>,
    severity: NotificationSeverity,
    #[serde(default)]
    read: bool,
    #[serde(default)]
    acted: bool,
    #[serde(default)]
    dismissed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NotificationEventStore {
    #[serde(default)]
    schema: String,
    #[serde(default)]
    entries: Vec<NotificationEventRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct NotificationEventRecord {
    id: String,
    notification_id: String,
    source_app: String,
    title: String,
    body: String,
    #[serde(default)]
    action_ref: Option<NotificationActionRef>,
    created_at: u64,
    disposition: NotificationEventDisposition,
    #[serde(default)]
    resolution: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum NotificationEventDisposition {
    Appeared,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct NativeChatRelayState {
    #[serde(default)]
    processed_event_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatHistoryMessage {
    sender_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sender_session_id: Option<String>,
    sender_nick: String,
    content: String,
    ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_ts: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signature: Option<String>,
}

pub fn sync_room_notifications(data_dir: &Path, summary: &RoomSummary) -> anyhow::Result<()> {
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    let existing_ids = store
        .entries
        .iter()
        .map(|entry| entry.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    let now = now_ts();
    let managed_ids = summary
        .pending_requests
        .iter()
        .map(|request| format!("room-pair-request:{}", request.request_id))
        .collect::<Vec<_>>();

    store.entries.retain(|entry| {
        if entry.kind != ROOM_PAIR_REQUEST_KIND {
            return entry.expires_at.is_none_or(|expires_at| expires_at > now);
        }
        managed_ids.iter().any(|id| id == &entry.id)
            && entry.expires_at.is_none_or(|expires_at| expires_at > now)
    });

    let room_title = if summary.room_control.title.trim().is_empty() {
        "Room".to_string()
    } else {
        summary.room_control.title.clone()
    };

    for request in &summary.pending_requests {
        let id = format!("room-pair-request:{}", request.request_id);
        let expires_at = Some(request.requested_at + ROOM_PAIRING_TTL_SECS);
        if let Some(existing) = store.entries.iter_mut().find(|entry| entry.id == id) {
            existing.title = format!("{} wants to pair", request.display_name);
            existing.body = format!(
                "{} on {} wants to join {}.",
                request.display_name, request.device_label, room_title
            );
            existing.action_ref = Some(NotificationActionRef {
                app: summary.room_slug.clone(),
                action_id: format!("room-approve-request:{}", request.request_id),
            });
            existing.expires_at = expires_at;
            existing.severity = NotificationSeverity::Attention;
            existing.source_app = summary.room_slug.clone();
            continue;
        }

        store.entries.push(NotificationEntryRecord {
            id: id.clone(),
            source_app: summary.room_slug.clone(),
            kind: ROOM_PAIR_REQUEST_KIND.to_string(),
            title: format!("{} wants to pair", request.display_name),
            body: format!(
                "{} on {} wants to join {}.",
                request.display_name, request.device_label, room_title
            ),
            action_ref: Some(NotificationActionRef {
                app: summary.room_slug.clone(),
                action_id: format!("room-approve-request:{}", request.request_id),
            }),
            created_at: request.requested_at,
            expires_at,
            severity: NotificationSeverity::Attention,
            read: false,
            acted: false,
            dismissed: false,
        });
        if !existing_ids.contains(&id) {
            record_event(
                data_dir,
                NotificationEventRecord {
                    id: format!("appeared:{id}"),
                    notification_id: id.clone(),
                    source_app: summary.room_slug.clone(),
                    title: format!("{} wants to pair", request.display_name),
                    body: format!(
                        "{} on {} wants to join {}.",
                        request.display_name, request.device_label, room_title
                    ),
                    action_ref: Some(NotificationActionRef {
                        app: summary.room_slug.clone(),
                        action_id: format!("room-approve-request:{}", request.request_id),
                    }),
                    created_at: request.requested_at,
                    disposition: NotificationEventDisposition::Appeared,
                    resolution: None,
                },
            )?;
        }
    }

    write_json_atomic(&path, &store)?;
    sync_native_chat_relay(data_dir)
}

pub fn load_summary(data_dir: &Path) -> anyhow::Result<NotificationSummary> {
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    let now = now_ts();
    store
        .entries
        .retain(|entry| entry.expires_at.is_none_or(|expires_at| expires_at > now));

    let entries = store
        .entries
        .iter()
        .filter(|entry| !entry.dismissed && !entry.acted)
        .map(|entry| NotificationEntryView {
            id: entry.id.clone(),
            source_app: entry.source_app.clone(),
            kind: entry.kind.clone(),
            title: entry.title.clone(),
            body: entry.body.clone(),
            action_ref: entry.action_ref.clone(),
            created_at: entry.created_at,
            expires_at: entry.expires_at,
            severity: entry.severity,
            read: entry.read,
            acted: entry.acted,
            dismissed: entry.dismissed,
        })
        .collect::<Vec<_>>();

    Ok(NotificationSummary {
        unread_count: entries.iter().filter(|entry| !entry.read).count(),
        attention_count: entries
            .iter()
            .filter(|entry| entry.severity == NotificationSeverity::Attention)
            .count(),
        entries,
    })
}

pub fn mark_acted_for_action(data_dir: &Path, action_id: &str) -> anyhow::Result<usize> {
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    let mut updated = 0usize;
    for entry in &mut store.entries {
        if entry
            .action_ref
            .as_ref()
            .map(|action_ref| action_ref.action_id.as_str())
            == Some(action_id)
            && !entry.acted
        {
            record_event(
                data_dir,
                NotificationEventRecord {
                    id: format!("resolved:{}", entry.id),
                    notification_id: entry.id.clone(),
                    source_app: entry.source_app.clone(),
                    title: entry.title.clone(),
                    body: entry.body.clone(),
                    action_ref: entry.action_ref.clone(),
                    created_at: now_ts(),
                    disposition: NotificationEventDisposition::Resolved,
                    resolution: Some("acted".to_string()),
                },
            )?;
            entry.acted = true;
            entry.read = true;
            updated += 1;
        }
    }

    if updated > 0 {
        write_json_atomic(&path, &store)?;
        sync_native_chat_relay(data_dir)?;
    }
    Ok(updated)
}

pub fn mark_read(data_dir: &Path, id: &str) -> anyhow::Result<bool> {
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    let mut updated = false;
    for entry in &mut store.entries {
        if entry.id == id && !entry.read {
            entry.read = true;
            updated = true;
        }
    }

    if updated {
        write_json_atomic(&path, &store)?;
        sync_native_chat_relay(data_dir)?;
    }
    Ok(updated)
}

pub fn dismiss(data_dir: &Path, id: &str) -> anyhow::Result<bool> {
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    let mut updated = false;
    for entry in &mut store.entries {
        if entry.id == id && !entry.dismissed {
            record_event(
                data_dir,
                NotificationEventRecord {
                    id: format!("resolved:{}", entry.id),
                    notification_id: entry.id.clone(),
                    source_app: entry.source_app.clone(),
                    title: entry.title.clone(),
                    body: entry.body.clone(),
                    action_ref: entry.action_ref.clone(),
                    created_at: now_ts(),
                    disposition: NotificationEventDisposition::Resolved,
                    resolution: Some("dismissed".to_string()),
                },
            )?;
            entry.dismissed = true;
            entry.read = true;
            updated = true;
        }
    }

    if updated {
        write_json_atomic(&path, &store)?;
        sync_native_chat_relay(data_dir)?;
    }
    Ok(updated)
}

fn notifications_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let root_dir = notifications_root_dir(data_dir)?;
    Ok(root_dir.join(NOTIFICATIONS_FILE))
}

fn notification_events_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let root_dir = notifications_root_dir(data_dir)?;
    Ok(root_dir.join(NOTIFICATION_EVENTS_FILE))
}

fn notifications_root_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, NOTIFICATIONS_ROOT_URI)
        .context("failed to resolve notifications root")
}

fn native_chat_relay_path(data_dir: &Path) -> anyhow::Result<PathBuf> {
    Ok(notifications_root_dir(data_dir)?.join(NATIVE_CHAT_RELAY_STATE_FILE))
}

fn native_chat_root_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    rooted_localhost_fs_path(data_dir, NATIVE_CHAT_ROOT_URI)
        .context("failed to resolve native chat relay root")
}

fn record_event(data_dir: &Path, event: NotificationEventRecord) -> anyhow::Result<()> {
    let path = notification_events_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationEventStore>(&path)?;
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATION_EVENTS_SCHEMA.to_string();
    }
    if store.entries.iter().any(|entry| entry.id == event.id) {
        return Ok(());
    }
    store.entries.push(event);
    if store.entries.len() > 4096 {
        store.entries = store.entries.split_off(store.entries.len() - 4096);
    }
    write_json_atomic(&path, &store)
}

fn sync_native_chat_relay(data_dir: &Path) -> anyhow::Result<()> {
    let relay_path = native_chat_relay_path(data_dir)?;
    let mut relay = read_json_or_default::<NativeChatRelayState>(&relay_path)?;
    let mut processed = relay
        .processed_event_ids
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let event_store =
        read_json_or_default::<NotificationEventStore>(&notification_events_path(data_dir)?)?;

    let chat_root = native_chat_root_dir(data_dir)?;
    let channels_path = chat_root.join("chat/channels.json");
    let mut channels = read_json_or_default::<Vec<String>>(&channels_path)?;
    if !channels
        .iter()
        .any(|channel| channel == NATIVE_CHAT_RELAY_CHANNEL)
    {
        channels.push(NATIVE_CHAT_RELAY_CHANNEL.to_string());
        write_json_atomic(&channels_path, &channels)?;
    }

    let history_path = chat_root.join(format!("chat/history/{}.json", NATIVE_CHAT_RELAY_CHANNEL));
    let mut history = read_json_or_default::<Vec<ChatHistoryMessage>>(&history_path)?;
    let mut changed = false;
    for event in &event_store.entries {
        if !processed.insert(event.id.clone()) {
            continue;
        }
        history.push(render_native_chat_event(event));
        changed = true;
    }

    if changed {
        if history.len() > 1000 {
            history = history.split_off(history.len() - 1000);
        }
        relay.processed_event_ids = processed.into_iter().rev().take(4096).collect::<Vec<_>>();
        relay.processed_event_ids.reverse();
        write_json_atomic(&history_path, &history)?;
        write_json_atomic(&relay_path, &relay)?;
    }

    Ok(())
}

fn render_native_chat_event(event: &NotificationEventRecord) -> ChatHistoryMessage {
    let content = match event.disposition {
        NotificationEventDisposition::Appeared => {
            format!(
                "[{}] {} Open PC2 Inbox to review.",
                source_label(&event.source_app),
                event.body
            )
        }
        NotificationEventDisposition::Resolved => {
            let suffix = event
                .resolution
                .as_deref()
                .map(|resolution| format!(" ({resolution})"))
                .unwrap_or_default();
            format!(
                "[{}] {} Resolved in PC2{}.",
                source_label(&event.source_app),
                event.title,
                suffix
            )
        }
    };

    ChatHistoryMessage {
        sender_id: String::new(),
        sender_session_id: None,
        sender_nick: "*".to_string(),
        content,
        ts: now_ts(),
        display_ts: Some(now_ts()),
        signature: None,
    }
}

fn source_label(source_app: &str) -> &str {
    if source_app.trim().is_empty() {
        "PC2"
    } else {
        source_app
    }
}

fn read_json_or_default<T>(path: &Path) -> anyhow::Result<T>
where
    T: serde::de::DeserializeOwned + Default,
{
    if !path.exists() {
        return Ok(T::default());
    }
    let bytes = fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if bytes.is_empty() {
        return Ok(T::default());
    }
    serde_json::from_slice(&bytes).with_context(|| format!("invalid {}", path.display()))
}

fn write_json_atomic<T>(path: &Path, value: &T) -> anyhow::Result<()>
where
    T: Serialize,
{
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path missing parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("notifications")
    ));
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room_service::{PendingRequestView, RoomControlSummary, RoomSummary};

    fn sample_summary() -> RoomSummary {
        RoomSummary {
            room_slug: "room-browser".to_string(),
            room_control: RoomControlSummary {
                title: "Exec Room".to_string(),
                ..Default::default()
            },
            pending_requests: vec![PendingRequestView {
                request_id: "req-1".to_string(),
                display_name: "Alice".to_string(),
                device_label: "Phone".to_string(),
                requested_at: now_ts(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn sync_room_notifications_creates_attention_entry() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.unread_count, 1);
        assert_eq!(summary.attention_count, 1);
        assert_eq!(summary.entries[0].source_app, "room-browser");
        assert_eq!(summary.entries[0].kind, ROOM_PAIR_REQUEST_KIND);
        assert_eq!(
            summary.entries[0]
                .action_ref
                .as_ref()
                .map(|action_ref| action_ref.action_id.as_str()),
            Some("room-approve-request:req-1")
        );
    }

    #[test]
    fn sync_room_notifications_prunes_stale_pair_requests() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let empty = RoomSummary::default();
        sync_room_notifications(tmp.path(), &empty).unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        assert!(summary.entries.is_empty());
    }

    #[test]
    fn mark_acted_for_action_hides_notification_entry() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let updated = mark_acted_for_action(tmp.path(), "room-approve-request:req-1").unwrap();
        assert_eq!(updated, 1);
        let summary = load_summary(tmp.path()).unwrap();
        assert!(summary.entries.is_empty());
    }

    #[test]
    fn mark_read_updates_entry_state() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let updated = mark_read(tmp.path(), "room-pair-request:req-1").unwrap();
        assert!(updated);
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.unread_count, 0);
        assert!(summary.entries[0].read);
    }

    #[test]
    fn dismiss_hides_entry() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let updated = dismiss(tmp.path(), "room-pair-request:req-1").unwrap();
        assert!(updated);
        let summary = load_summary(tmp.path()).unwrap();
        assert!(summary.entries.is_empty());
    }

    #[test]
    fn sync_room_notifications_relays_into_native_chat_channel() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();

        let chat_root = native_chat_root_dir(tmp.path()).unwrap();
        let channels: Vec<String> =
            read_json_or_default(&chat_root.join("chat/channels.json")).unwrap();
        assert!(channels
            .iter()
            .any(|channel| channel == NATIVE_CHAT_RELAY_CHANNEL));

        let history: Vec<ChatHistoryMessage> =
            read_json_or_default(&chat_root.join("chat/history/!pc2.json")).unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].content.contains("Open PC2 Inbox to review."));
    }

    #[test]
    fn native_chat_relay_does_not_duplicate_existing_entries() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();

        let chat_root = native_chat_root_dir(tmp.path()).unwrap();
        let history: Vec<ChatHistoryMessage> =
            read_json_or_default(&chat_root.join("chat/history/!pc2.json")).unwrap();
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn dismiss_relays_resolution_into_native_chat_channel() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let updated = dismiss(tmp.path(), "room-pair-request:req-1").unwrap();
        assert!(updated);

        let chat_root = native_chat_root_dir(tmp.path()).unwrap();
        let history: Vec<ChatHistoryMessage> =
            read_json_or_default(&chat_root.join("chat/history/!pc2.json")).unwrap();
        assert_eq!(history.len(), 2);
        assert!(history[1].content.contains("Resolved in PC2"));
        assert!(history[1].content.contains("dismissed"));
    }

    #[test]
    fn sync_room_notifications_records_appeared_event() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();

        let events: NotificationEventStore =
            read_json_or_default(&notification_events_path(tmp.path()).unwrap()).unwrap();
        assert_eq!(events.entries.len(), 1);
        assert_eq!(
            events.entries[0].disposition,
            NotificationEventDisposition::Appeared
        );
        assert_eq!(events.entries[0].notification_id, "room-pair-request:req-1");
    }

    #[test]
    fn dismiss_records_resolved_event() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        dismiss(tmp.path(), "room-pair-request:req-1").unwrap();

        let events: NotificationEventStore =
            read_json_or_default(&notification_events_path(tmp.path()).unwrap()).unwrap();
        assert_eq!(events.entries.len(), 2);
        assert_eq!(
            events.entries[1].disposition,
            NotificationEventDisposition::Resolved
        );
        assert_eq!(events.entries[1].resolution.as_deref(), Some("dismissed"));
    }
}
