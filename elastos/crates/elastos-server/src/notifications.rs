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
const ROOM_ACCESS_REQUEST_KIND: &str = "room_access_request";
const ROOM_ACCESS_REQUEST_ID_PREFIX: &str = "room-access-request:";
const ROOM_ACCESS_REQUEST_TTL_SECS: u64 = 10 * 60;
const EXTERNAL_HTTP_REQUEST_KIND: &str = "external_http_request";
const CONTACT_REQUEST_KIND: &str = "contact_request";
const CONTACT_REQUEST_ID_PREFIX: &str = "contact-request:";
const DIRECT_MESSAGE_KIND: &str = "direct_message";
const DIRECT_MESSAGE_ID_PREFIX: &str = "direct-message:";
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

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NotificationSeverity {
    #[default]
    Info,
    Attention,
    Critical,
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
        .map(|request| room_access_request_notification_id(&request.request_id))
        .collect::<Vec<_>>();

    store.entries.retain(|entry| {
        if entry.kind != ROOM_ACCESS_REQUEST_KIND {
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
        let id = room_access_request_notification_id(&request.request_id);
        let expires_at = Some(request.requested_at + ROOM_ACCESS_REQUEST_TTL_SECS);
        if let Some(existing) = store.entries.iter_mut().find(|entry| entry.id == id) {
            existing.title = format!("{} wants to join Chat", request.display_name);
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
            kind: ROOM_ACCESS_REQUEST_KIND.to_string(),
            title: format!("{} wants to join Chat", request.display_name),
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
                    title: format!("{} wants to join Chat", request.display_name),
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

    // Command and verified incoming-event boundaries sync this projection, so
    // persist only a real change.
    let serialized = serde_json::to_vec_pretty(&store)?;
    let unchanged = match fs::read(&path) {
        Ok(existing) => existing == serialized,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => store.entries.is_empty(),
        Err(err) => return Err(err).with_context(|| format!("failed to read {}", path.display())),
    };
    if unchanged {
        return Ok(());
    }

    write_json_atomic(&path, &store)
}

/// A pending incoming contact request projected from the principal-scoped
/// contact store. This module never reads collaboration state itself.
#[derive(Debug, Clone)]
pub struct PendingContactRequestNotification {
    pub request_hash: String,
    pub display_name: String,
    pub handle: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
}

/// Projects pending incoming contact requests into one read response.
///
/// The contact store remains the only durable truth. Home and Inbox reads do
/// not copy these entries into the shared notification store, and a decided,
/// revoked, or expired request disappears from the next projection naturally.
pub fn project_contact_request_notifications(
    summary: &mut NotificationSummary,
    pending: &[PendingContactRequestNotification],
) {
    let now = now_ts();
    summary
        .entries
        .retain(|entry| entry.kind != CONTACT_REQUEST_KIND);
    let mut pending = pending
        .iter()
        .filter(|request| request.expires_at > now)
        .collect::<Vec<_>>();
    pending.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.request_hash.cmp(&right.request_hash))
    });
    for request in pending {
        let id = contact_request_notification_id(&request.request_hash);
        let title = format!("{} wants to connect", request.display_name);
        let body =
            contact_request_notification_body(&request.display_name, request.handle.as_deref());
        summary.entries.push(NotificationEntryView {
            id,
            source_app: "people".to_string(),
            kind: CONTACT_REQUEST_KIND.to_string(),
            title,
            body,
            action_ref: Some(NotificationActionRef {
                app: "inbox".to_string(),
                action_id: format!("contact-accept-request:{}", request.request_hash),
            }),
            created_at: request.created_at,
            expires_at: Some(request.expires_at),
            severity: NotificationSeverity::Attention,
            read: false,
            acted: false,
            dismissed: false,
        });
    }
    summary.unread_count = summary.entries.iter().filter(|entry| !entry.read).count();
    summary.attention_count = summary
        .entries
        .iter()
        .filter(|entry| entry.severity == NotificationSeverity::Attention)
        .count();
}

fn contact_request_notification_body(display_name: &str, handle: Option<&str>) -> String {
    match handle {
        Some(handle) => format!(
            "{display_name} (@{handle}) wants to add you as a contact. Accepting adds them to People."
        ),
        None => {
            format!("{display_name} wants to add you as a contact. Accepting adds them to People.")
        }
    }
}

fn contact_request_notification_id(request_hash: &str) -> String {
    format!("{CONTACT_REQUEST_ID_PREFIX}{request_hash}")
}

/// Records that a direct message arrived, one live entry per conversation.
/// A repeat message resurfaces the entry; reading the conversation in Chat
/// resolves it through [`mark_acted_for_action`] with the same action id.
/// The sender name is the signed Profile name the receive path verified —
/// this module never invents one.
pub fn upsert_direct_message_notification(
    data_dir: &Path,
    conversation_id: &str,
    sender_display_name: &str,
    now: u64,
) -> anyhow::Result<()> {
    let id = direct_message_notification_id(conversation_id);
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    let already_exists = store.entries.iter().any(|entry| entry.id == id);
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }
    let title = format!("New message from {sender_display_name}");
    let body = format!("{sender_display_name} sent you a message in Chat.");
    let action_ref = Some(NotificationActionRef {
        app: "chat-room".to_string(),
        action_id: direct_message_notification_action_id(conversation_id),
    });

    if let Some(existing) = store.entries.iter_mut().find(|entry| entry.id == id) {
        existing.source_app = "chat-room".to_string();
        existing.kind = DIRECT_MESSAGE_KIND.to_string();
        existing.title = title;
        existing.body = body;
        existing.action_ref = action_ref;
        existing.created_at = now;
        existing.severity = NotificationSeverity::Attention;
        existing.read = false;
        existing.acted = false;
        existing.dismissed = false;
        return write_json_atomic(&path, &store);
    }

    store.entries.push(NotificationEntryRecord {
        id: id.clone(),
        source_app: "chat-room".to_string(),
        kind: DIRECT_MESSAGE_KIND.to_string(),
        title: title.clone(),
        body: body.clone(),
        action_ref: action_ref.clone(),
        created_at: now,
        expires_at: None,
        severity: NotificationSeverity::Attention,
        read: false,
        acted: false,
        dismissed: false,
    });
    if !already_exists {
        record_event(
            data_dir,
            NotificationEventRecord {
                id: format!("appeared:{id}"),
                notification_id: id,
                source_app: "chat-room".to_string(),
                title,
                body,
                action_ref,
                created_at: now,
                disposition: NotificationEventDisposition::Appeared,
                resolution: None,
            },
        )?;
    }
    write_json_atomic(&path, &store)
}

/// The action id that resolves a conversation's message notification. Chat's
/// conversation read marks it acted; the entry's own action ref carries the
/// same id so an Inbox resolution and a Chat read converge on one record.
pub fn direct_message_notification_action_id(conversation_id: &str) -> String {
    format!("chat-open-direct:{conversation_id}")
}

fn direct_message_notification_id(conversation_id: &str) -> String {
    format!("{DIRECT_MESSAGE_ID_PREFIX}{conversation_id}")
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

pub fn upsert_external_http_request(
    data_dir: &Path,
    request_id: &str,
    source_app: &str,
    title: &str,
    body: &str,
    approve_action_id: &str,
    created_at: u64,
) -> anyhow::Result<()> {
    let id = external_http_request_notification_id(request_id);
    let path = notifications_path(data_dir)?;
    let mut store = read_json_or_default::<NotificationStore>(&path)?;
    let already_exists = store.entries.iter().any(|entry| entry.id == id);
    if store.schema.trim().is_empty() {
        store.schema = NOTIFICATIONS_SCHEMA.to_string();
    }

    if let Some(existing) = store.entries.iter_mut().find(|entry| entry.id == id) {
        existing.source_app = source_app.to_string();
        existing.kind = EXTERNAL_HTTP_REQUEST_KIND.to_string();
        existing.title = title.to_string();
        existing.body = body.to_string();
        existing.action_ref = Some(NotificationActionRef {
            app: source_app.to_string(),
            action_id: approve_action_id.to_string(),
        });
        existing.severity = NotificationSeverity::Attention;
        existing.dismissed = false;
        existing.acted = false;
        write_json_atomic(&path, &store)?;
        return Ok(());
    }

    store.entries.push(NotificationEntryRecord {
        id: id.clone(),
        source_app: source_app.to_string(),
        kind: EXTERNAL_HTTP_REQUEST_KIND.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        action_ref: Some(NotificationActionRef {
            app: source_app.to_string(),
            action_id: approve_action_id.to_string(),
        }),
        created_at,
        expires_at: None,
        severity: NotificationSeverity::Attention,
        read: false,
        acted: false,
        dismissed: false,
    });
    if !already_exists {
        record_event(
            data_dir,
            NotificationEventRecord {
                id: format!("appeared:{id}"),
                notification_id: id,
                source_app: source_app.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                action_ref: Some(NotificationActionRef {
                    app: source_app.to_string(),
                    action_id: approve_action_id.to_string(),
                }),
                created_at,
                disposition: NotificationEventDisposition::Appeared,
                resolution: None,
            },
        )?;
    }
    write_json_atomic(&path, &store)
}

pub fn dismiss_external_http_request(data_dir: &Path, request_id: &str) -> anyhow::Result<bool> {
    dismiss(data_dir, &external_http_request_notification_id(request_id))
}

fn external_http_request_notification_id(request_id: &str) -> String {
    format!("external-http-request:{request_id}")
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

fn room_access_request_notification_id(request_id: &str) -> String {
    format!("{ROOM_ACCESS_REQUEST_ID_PREFIX}{request_id}")
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
            room_slug: "chat-room".to_string(),
            room_control: RoomControlSummary {
                title: "Exec Room".to_string(),
                ..Default::default()
            },
            pending_requests: vec![PendingRequestView {
                request_id: "req-1".to_string(),
                display_name: "Alice".to_string(),
                device_label: "Phone".to_string(),
                requested_at: now_ts(),
                capabilities: crate::room_service::room_access_capabilities(),
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
        assert_eq!(summary.entries[0].source_app, "chat-room");
        assert_eq!(summary.entries[0].id, "room-access-request:req-1");
        assert_eq!(summary.entries[0].kind, ROOM_ACCESS_REQUEST_KIND);
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
        let updated = mark_read(tmp.path(), "room-access-request:req-1").unwrap();
        assert!(updated);
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.unread_count, 0);
        assert!(summary.entries[0].read);
    }

    #[test]
    fn dismiss_hides_entry() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        let updated = dismiss(tmp.path(), "room-access-request:req-1").unwrap();
        assert!(updated);
        let summary = load_summary(tmp.path()).unwrap();
        assert!(summary.entries.is_empty());
    }

    #[test]
    fn contact_request_notification_explains_the_people_consequence_only() {
        assert_eq!(
            contact_request_notification_body("Morgan", None),
            "Morgan wants to add you as a contact. Accepting adds them to People."
        );
        assert_eq!(
            contact_request_notification_body("Morgan", Some("morgan")),
            "Morgan (@morgan) wants to add you as a contact. Accepting adds them to People."
        );
    }

    #[test]
    fn contact_request_notifications_are_pure_inbox_projections() {
        let tmp = tempfile::tempdir().unwrap();
        let now = now_ts();
        let pending = vec![PendingContactRequestNotification {
            request_hash: "sha256:req".to_string(),
            display_name: "Morgan".to_string(),
            handle: Some("morgan".to_string()),
            created_at: now,
            expires_at: now + 600,
        }];
        let mut summary = load_summary(tmp.path()).unwrap();
        project_contact_request_notifications(&mut summary, &pending);
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.attention_count, 1);
        let entry = &summary.entries[0];
        assert_eq!(entry.kind, CONTACT_REQUEST_KIND);
        assert_eq!(entry.id, "contact-request:sha256:req");
        assert_eq!(entry.source_app, "people");
        assert_eq!(entry.title, "Morgan wants to connect");
        // The notification points at Inbox and never carries the decision:
        // its action is the Inbox review reference, resolved by Inbox alone.
        assert_eq!(
            entry.action_ref,
            Some(NotificationActionRef {
                app: "inbox".to_string(),
                action_id: "contact-accept-request:sha256:req".to_string(),
            })
        );
        assert_eq!(entry.expires_at, Some(now + 600));
        assert!(!notifications_path(tmp.path()).unwrap().exists());

        // Re-projecting replaces the dynamic row instead of duplicating it.
        project_contact_request_notifications(&mut summary, &pending);
        assert_eq!(summary.entries.len(), 1);

        // A decided (or revoked, or expired) request leaves the pending
        // projection without mutating a second durable store.
        project_contact_request_notifications(&mut summary, &[]);
        assert!(summary.entries.is_empty());
        assert!(!notifications_path(tmp.path()).unwrap().exists());
    }

    #[test]
    fn direct_message_notification_resolves_on_read_and_resurfaces_on_new_mail() {
        let tmp = tempfile::tempdir().unwrap();
        let conversation = "direct:sha256:abc";
        upsert_direct_message_notification(tmp.path(), conversation, "Alice", 100).unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].kind, DIRECT_MESSAGE_KIND);
        assert_eq!(summary.entries[0].id, "direct-message:direct:sha256:abc");
        assert_eq!(summary.entries[0].title, "New message from Alice");
        assert_eq!(summary.entries[0].source_app, "chat-room");
        assert_eq!(
            summary.entries[0]
                .action_ref
                .as_ref()
                .map(|action| action.action_id.as_str()),
            Some("chat-open-direct:direct:sha256:abc")
        );

        // A second message keeps one entry per conversation.
        upsert_direct_message_notification(tmp.path(), conversation, "Alice", 101).unwrap();
        assert_eq!(load_summary(tmp.path()).unwrap().entries.len(), 1);

        // Reading the conversation resolves it.
        let acted = mark_acted_for_action(
            tmp.path(),
            &direct_message_notification_action_id(conversation),
        )
        .unwrap();
        assert_eq!(acted, 1);
        assert!(load_summary(tmp.path()).unwrap().entries.is_empty());

        // New mail after a read resurfaces the same entry, unread again.
        upsert_direct_message_notification(tmp.path(), conversation, "Alice Renamed", 102).unwrap();
        let resurfaced = load_summary(tmp.path()).unwrap();
        assert_eq!(resurfaced.entries.len(), 1);
        assert!(!resurfaced.entries[0].read);
        assert_eq!(
            resurfaced.entries[0].title,
            "New message from Alice Renamed"
        );
    }

    #[test]
    fn sync_room_notifications_does_not_write_shared_native_chat_state() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();

        let shared_native_chat = tmp
            .path()
            .join("localhost/Users/self/.AppData/LocalHost/Chat");
        assert!(!shared_native_chat.exists());
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
        assert_eq!(
            events.entries[0].notification_id,
            "room-access-request:req-1"
        );
    }

    #[test]
    fn dismiss_records_resolved_event() {
        let tmp = tempfile::tempdir().unwrap();
        sync_room_notifications(tmp.path(), &sample_summary()).unwrap();
        dismiss(tmp.path(), "room-access-request:req-1").unwrap();

        let events: NotificationEventStore =
            read_json_or_default(&notification_events_path(tmp.path()).unwrap()).unwrap();
        assert_eq!(events.entries.len(), 2);
        assert_eq!(
            events.entries[1].disposition,
            NotificationEventDisposition::Resolved
        );
        assert_eq!(events.entries[1].resolution.as_deref(), Some("dismissed"));
    }
    #[test]
    fn external_http_request_appears_once_and_can_be_dismissed() {
        let tmp = tempfile::tempdir().unwrap();
        upsert_external_http_request(
            tmp.path(),
            "wallet-prices",
            "wallet",
            "Wallet requests market prices",
            "Wallet wants approved HTTP access to CoinGecko.",
            "wallet-price-http-approve:coingecko",
            42,
        )
        .unwrap();
        upsert_external_http_request(
            tmp.path(),
            "wallet-prices",
            "wallet",
            "Wallet requests market prices",
            "Wallet wants approved HTTP access to CoinGecko.",
            "wallet-price-http-approve:coingecko",
            43,
        )
        .unwrap();

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.entries.len(), 1);
        assert_eq!(summary.entries[0].kind, EXTERNAL_HTTP_REQUEST_KIND);
        assert_eq!(
            summary.entries[0]
                .action_ref
                .as_ref()
                .map(|action| action.action_id.as_str()),
            Some("wallet-price-http-approve:coingecko")
        );

        assert!(dismiss_external_http_request(tmp.path(), "wallet-prices").unwrap());
        assert!(load_summary(tmp.path()).unwrap().entries.is_empty());
    }
}
