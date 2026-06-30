use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use base64::engine::general_purpose::{
    STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD as BASE64_URL_SAFE_NO_PAD,
};
use base64::Engine as _;
use elastos_common::localhost::rooted_localhost_fs_path;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use url::Url;

const STATE_SCHEMA: &str = "elastos.room.state.v1";
const ROOM_SLUG: &str = "chat-room";
const ROOM_ROOT_URI: &str = "localhost://Local/Shared/AppCapsules/chat-room";
const ROOM_SHARED_DIR: &str = "room";
const ROOM_LOCAL_DIR: &str = "local";
const ROOM_META_FILE: &str = "room.json";
const ROOM_CONTROL_FILE: &str = "control.json";
const ROOM_MEMBERS_FILE: &str = "members.json";
const ROOM_INVITES_FILE: &str = "invites.json";
const ROOM_KEY_EPOCHS_FILE: &str = "key-epochs.json";
const BROWSER_ACCESS_REQUESTS_FILE: &str = "browser-access-requests.json";
const ROOM_SESSIONS_FILE: &str = "sessions.json";
const ROOM_OBJECTS_FILE: &str = "objects.json";
const ROOM_UPLOADS_FILE: &str = "uploads.json";
const ROOM_ATTACHMENTS_DIR: &str = "attachments";
const ROOM_UPLOADS_DIR: &str = "uploads";
const ROOM_LOCK_FILE: &str = "state.lock";
const ROOM_INVITE_ENVELOPE_SCHEMA: &str = "elastos.room.invite.v1";
const ROOM_INVITE_ENVELOPE_DOMAIN: &str = "elastos.room.invite.v1";
const ROOM_JOIN_INVITE_SCHEMA: &str = "elastos.room.join-invite.v1";
const ROOM_JOIN_INVITE_DOMAIN: &str = "elastos.room.join-invite.v1";
const ROOM_ACCEPT_ENVELOPE_SCHEMA: &str = "elastos.room.accept.v1";
const ROOM_ACCEPT_ENVELOPE_DOMAIN: &str = "elastos.room.accept.v1";
const ROOM_OBJECT_ENVELOPE_SCHEMA: &str = "elastos.room.object.v1";
const BROWSER_ACCESS_REQUEST_TTL_SECS: u64 = 10 * 60;
const SESSION_TTL_SECS: u64 = 12 * 60 * 60;
const UPLOAD_TTL_SECS: u64 = 15 * 60;
const INVITE_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const MAX_OBJECTS: usize = 500;
const MAX_OBJECT_BODY_LEN: usize = 2_000;
const MAX_ATTACHMENT_BYTES: usize = 8 * 1024 * 1024;
pub const ATTACHMENT_UPLOAD_CHUNK_BYTES: usize = 256 * 1024;
pub const ROOM_ACCESS_CAPABILITY: &str = "room.access";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomSummary {
    pub room_slug: String,
    pub pending_count: usize,
    pub active_session_count: usize,
    pub latest_request_name: Option<String>,
    pub latest_request_device: Option<String>,
    #[serde(default)]
    pub active_participants: Vec<ParticipantView>,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequestView>,
    #[serde(default)]
    pub active_sessions: Vec<ActiveSessionView>,
    #[serde(default)]
    pub room_control: RoomControlSummary,
    #[serde(default)]
    pub local_runtime_did: Option<String>,
    #[serde(default)]
    pub local_runtime_role: Option<RoomRole>,
    #[serde(default)]
    pub canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    pub ephemeral_hosted_guest_url: Option<String>,
    #[serde(default = "summary_browser_access_allowed_default")]
    pub browser_access_allowed: bool,
    #[serde(default)]
    pub browser_access_block_reason: Option<String>,
    #[serde(default)]
    pub transport: RoomTransportView,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAccessRequestOutput {
    pub request_id: String,
    pub room_slug: String,
    pub status: String,
    pub requested_at: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserAccessStatusOutput {
    pub request_id: String,
    pub room_slug: String,
    pub status: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub expires_at: Option<u64>,
    #[serde(default)]
    pub denial_reason: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalRuntimeSessionOutput {
    pub token: String,
    pub display_name: String,
    pub expires_at: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LocalRuntimeSessionWithTransportOutput {
    pub session: LocalRuntimeSessionOutput,
    pub transport_envelope: Option<RoomObjectEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionView {
    pub room_slug: String,
    pub display_name: String,
    pub expires_at: u64,
    pub latest_seq: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub participants: Vec<ParticipantView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationObjectView {
    pub seq: u64,
    pub sender: String,
    #[serde(default)]
    pub sender_member_did: Option<String>,
    #[serde(default)]
    pub from_current_session: bool,
    pub kind: ConversationObjectKind,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub link: Option<LinkPreviewView>,
    #[serde(default)]
    pub attachment: Option<AttachmentView>,
    pub created_at: u64,
}

#[derive(Debug, Clone)]
pub struct AppendedConversationObject {
    pub object: ConversationObjectView,
    pub sender_member_did: Option<String>,
    pub transport_envelope: Option<RoomObjectEnvelope>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParticipantView {
    pub display_name: String,
    pub device_label: String,
    pub last_seen_at: u64,
    #[serde(default)]
    pub member_did: Option<String>,
    #[serde(default)]
    pub role: Option<RoomRole>,
    #[serde(default)]
    pub local_session_count: usize,
    #[serde(default)]
    pub is_current_session: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingRequestView {
    pub request_id: String,
    pub display_name: String,
    pub device_label: String,
    pub requested_at: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveSessionView {
    pub session_id: String,
    pub token: String,
    pub display_name: String,
    pub device_label: String,
    pub approved_at: u64,
    pub expires_at: u64,
    pub last_seen_at: u64,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub member_did: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomControlSummary {
    pub title: String,
    #[serde(default)]
    pub owner_did: Option<String>,
    pub current_key_epoch: u64,
    #[serde(default)]
    pub admin_count: usize,
    #[serde(default)]
    pub member_count: usize,
    #[serde(default)]
    pub active_member_count: usize,
    #[serde(default)]
    pub access_policy: RoomAccessPolicyView,
    #[serde(default)]
    pub members: Vec<RoomMemberView>,
    #[serde(default)]
    pub pending_invites: Vec<RoomInviteView>,
    #[serde(default)]
    pub key_epochs: Vec<RoomKeyEpochView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomAccessPolicyView {
    #[serde(default = "room_access_policy_enabled_default")]
    pub allow_guest_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    pub allow_member_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    pub allow_members_to_host_guests: bool,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RoomRole {
    Owner,
    Admin,
    Member,
}

impl Default for RoomRole {
    fn default() -> Self {
        Self::Member
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMemberView {
    pub member_did: String,
    pub role: RoomRole,
    pub added_at: u64,
    pub added_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_card: Option<RoomProfileCardView>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomProfileCardView {
    pub schema: String,
    pub profile_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle: Option<String>,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInviteView {
    pub invite_id: String,
    pub invited_did: String,
    pub role: RoomRole,
    pub invited_by: String,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomKeyEpochView {
    pub epoch: u64,
    pub created_at: u64,
    pub created_by: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationFeed {
    pub room_slug: String,
    pub latest_seq: u64,
    pub objects: Vec<ConversationObjectView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomPollView {
    pub room_slug: String,
    pub display_name: String,
    pub expires_at: u64,
    pub latest_seq: u64,
    #[serde(default)]
    pub participants: Vec<ParticipantView>,
    pub objects: Vec<ConversationObjectView>,
    #[serde(default)]
    pub transport: RoomTransportView,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RoomTransportView {
    #[serde(default)]
    pub available: bool,
    #[serde(default)]
    pub connected_peer_count: usize,
    #[serde(default)]
    pub topic: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConversationObjectKind {
    System,
    Text,
    Emoji,
    Link,
    Attachment,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomObjectEnvelope {
    pub schema: String,
    pub room_slug: String,
    pub event_id: String,
    pub sender: String,
    pub sender_member_did: String,
    pub kind: ConversationObjectKind,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub link: Option<LinkPreviewView>,
    #[serde(default)]
    pub attachment: Option<AttachmentView>,
    #[serde(default)]
    pub attachment_bytes_b64: Option<String>,
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkPreviewView {
    pub url: String,
    pub host: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentView {
    pub attachment_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub is_image: bool,
    pub is_audio: bool,
    pub is_video: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentUploadStartOutput {
    pub upload_id: String,
    pub chunk_size_bytes: usize,
    pub received_bytes: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachmentUploadChunkOutput {
    pub upload_id: String,
    pub received_bytes: u64,
    pub size_bytes: u64,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct BrowserAccessRequestInput {
    pub display_name: String,
    pub device_label: String,
    pub host_member_did: Option<String>,
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalRuntimeAccess {
    #[serde(default)]
    pub runtime_did: Option<String>,
    #[serde(default)]
    pub member_role: Option<RoomRole>,
    pub browser_access_allowed: bool,
    #[serde(default)]
    pub block_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalOutcome {
    pub request_id: String,
    pub display_name: String,
    pub device_label: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DenyOutcome {
    pub request_id: String,
    pub display_name: String,
    pub device_label: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeOutcome {
    pub revoked_count: usize,
    #[serde(default)]
    pub revoked_participants: Vec<ParticipantView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevokeSessionOutcome {
    pub token: String,
    pub display_name: String,
    pub device_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomOwnerSeedInput {
    pub owner_did: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInviteInput {
    pub actor_did: String,
    pub invited_did: String,
    pub role: RoomRole,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomInviteAcceptInput {
    pub actor_did: String,
    pub invite_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomJoinInviteInput {
    pub actor_did: String,
    pub issuer_gateway: String,
    pub inviter_profile: Option<RoomProfileCardView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomJoinInviteView {
    pub token: String,
    pub invite_url: String,
    pub issuer_gateway: String,
    pub room_title: String,
    pub invited_by: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomJoinInviteEnvelope {
    pub payload: SignedRoomJoinInvitePayload,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomJoinInvitePayload {
    pub schema: String,
    pub room_slug: String,
    pub room_title: String,
    pub issuer_gateway: String,
    pub invited_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inviter_profile: Option<RoomProfileCardView>,
    pub role: RoomRole,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomInviteEnvelope {
    pub payload: SignedRoomInvitePayload,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomInvitePayload {
    pub schema: String,
    pub room_slug: String,
    pub room_title: String,
    #[serde(default)]
    pub owner_did: Option<String>,
    pub current_key_epoch: u64,
    pub invite_id: String,
    pub invited_did: String,
    pub role: RoomRole,
    pub invited_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inviter_profile: Option<RoomProfileCardView>,
    pub created_at: u64,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomAcceptEnvelope {
    pub payload: SignedRoomAcceptPayload,
    pub signature: String,
    pub signer_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedRoomAcceptPayload {
    pub schema: String,
    pub room_slug: String,
    pub room_title: String,
    #[serde(default)]
    pub owner_did: Option<String>,
    pub current_key_epoch: u64,
    pub invite_id: String,
    pub member_did: String,
    pub role: RoomRole,
    pub invited_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_profile: Option<RoomProfileCardView>,
    pub accepted_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomMemberRemoveInput {
    pub actor_did: String,
    pub member_did: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomKeyRotateInput {
    pub actor_did: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomResetOutput {
    pub room_slug: String,
    pub cleared_requests: usize,
    pub cleared_sessions: usize,
    pub cleared_objects: usize,
    pub cleared_uploads: usize,
    pub cleared_attachments: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomAccessPolicyUpdateInput {
    pub actor_did: String,
    pub allow_guest_invites: bool,
    pub allow_member_invites: bool,
    pub allow_members_to_host_guests: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RoomMeta {
    schema: String,
    room_slug: String,
    next_seq: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoomControlRecord {
    schema: String,
    room_slug: String,
    title: String,
    created_at: u64,
    updated_at: u64,
    current_key_epoch: u64,
    #[serde(default)]
    owner_did: Option<String>,
    #[serde(default = "room_access_policy_enabled_default")]
    allow_guest_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    allow_member_invites: bool,
    #[serde(default = "room_access_policy_enabled_default")]
    allow_members_to_host_guests: bool,
}

impl Default for RoomControlRecord {
    fn default() -> Self {
        Self {
            schema: String::new(),
            room_slug: String::new(),
            title: String::new(),
            created_at: 0,
            updated_at: 0,
            current_key_epoch: 0,
            owner_did: None,
            allow_guest_invites: true,
            allow_member_invites: true,
            allow_members_to_host_guests: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RoomMemberRecord {
    member_did: String,
    role: RoomRole,
    added_at: u64,
    added_by: String,
    active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_card: Option<RoomProfileCardView>,
    #[serde(default)]
    removed_at: Option<u64>,
    #[serde(default)]
    removed_by: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RoomInviteRecord {
    invite_id: String,
    invited_did: String,
    role: RoomRole,
    invited_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    inviter_profile: Option<RoomProfileCardView>,
    created_at: u64,
    expires_at: u64,
    status: InviteStatus,
    #[serde(default)]
    acted_at: Option<u64>,
    #[serde(default)]
    acted_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum InviteStatus {
    Pending,
    Accepted,
    Revoked,
    Expired,
}

impl Default for InviteStatus {
    fn default() -> Self {
        Self::Pending
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RoomKeyEpochRecord {
    epoch: u64,
    created_at: u64,
    created_by: String,
    reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RoomState {
    schema: String,
    room_slug: String,
    next_seq: u64,
    control: RoomControlRecord,
    #[serde(default)]
    members: Vec<RoomMemberRecord>,
    #[serde(default)]
    invites: Vec<RoomInviteRecord>,
    #[serde(default)]
    key_epochs: Vec<RoomKeyEpochRecord>,
    #[serde(default)]
    pending_requests: Vec<BrowserAccessRequestRecord>,
    #[serde(default)]
    sessions: Vec<SessionRecord>,
    #[serde(default)]
    objects: Vec<ConversationObjectRecord>,
    #[serde(default)]
    uploads: Vec<UploadRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BrowserAccessRequestRecord {
    request_id: String,
    display_name: String,
    device_label: String,
    #[serde(default)]
    host_member_did: Option<String>,
    #[serde(default = "default_room_access_capabilities")]
    capabilities: Vec<String>,
    requested_at: u64,
    expires_at: u64,
    status: BrowserAccessStatus,
    #[serde(default)]
    denial_reason: Option<String>,
    #[serde(default)]
    session_token: Option<String>,
    #[serde(default)]
    session_expires_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum BrowserAccessStatus {
    Pending,
    Approved,
    Denied,
    Expired,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionRecord {
    token: String,
    #[serde(default)]
    actor_id: String,
    display_name: String,
    device_label: String,
    #[serde(default)]
    member_did: Option<String>,
    #[serde(default = "default_room_access_capabilities")]
    capabilities: Vec<String>,
    approved_at: u64,
    expires_at: u64,
    last_seen_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationObjectRecord {
    seq: u64,
    #[serde(default)]
    event_id: String,
    sender: String,
    #[serde(default)]
    sender_member_did: Option<String>,
    #[serde(default)]
    sender_actor_id: String,
    kind: ConversationObjectKind,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    emoji: Option<String>,
    #[serde(default)]
    link: Option<LinkPreviewView>,
    #[serde(default)]
    attachment: Option<AttachmentView>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UploadRecord {
    upload_id: String,
    token: String,
    file_name: String,
    mime_type: String,
    size_bytes: u64,
    received_bytes: u64,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug, Clone)]
struct CurrentSessionIdentity {
    actor_id: String,
    member_did: Option<String>,
}

impl Default for RoomState {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA.to_string(),
            room_slug: ROOM_SLUG.to_string(),
            next_seq: 1,
            control: RoomControlRecord::default(),
            members: Vec::new(),
            invites: Vec::new(),
            key_epochs: Vec::new(),
            pending_requests: Vec::new(),
            sessions: Vec::new(),
            objects: Vec::new(),
            uploads: Vec::new(),
        }
    }
}

pub fn room_access_capabilities() -> Vec<String> {
    default_room_access_capabilities()
}

fn default_room_access_capabilities() -> Vec<String> {
    vec![ROOM_ACCESS_CAPABILITY.to_string()]
}

#[derive(Debug, Clone)]
struct RoomPaths {
    root_dir: PathBuf,
    room_dir: PathBuf,
    local_dir: PathBuf,
    lock_path: PathBuf,
    room_meta_path: PathBuf,
    control_path: PathBuf,
    members_path: PathBuf,
    invites_path: PathBuf,
    key_epochs_path: PathBuf,
    pair_requests_path: PathBuf,
    sessions_path: PathBuf,
    objects_path: PathBuf,
    uploads_path: PathBuf,
    attachments_dir: PathBuf,
    uploads_dir: PathBuf,
}

pub fn room_slug() -> &'static str {
    ROOM_SLUG
}

pub fn room_root_uri() -> &'static str {
    ROOM_ROOT_URI
}

pub fn load_summary(data_dir: &Path) -> anyhow::Result<RoomSummary> {
    with_locked_state(data_dir, |_, state| {
        let next_pending = next_pending_request(state);
        let active_participants = participant_views_from_state(state, None);
        Ok(RoomSummary {
            room_slug: state.room_slug.clone(),
            pending_count: state
                .pending_requests
                .iter()
                .filter(|item| item.status == BrowserAccessStatus::Pending)
                .count(),
            active_session_count: state.sessions.len(),
            latest_request_name: next_pending.map(|item| item.display_name.clone()),
            latest_request_device: next_pending.map(|item| item.device_label.clone()),
            active_participants,
            pending_requests: pending_request_views_from_state(state),
            active_sessions: active_session_views_from_state(state),
            room_control: room_control_summary_from_state(state),
            local_runtime_did: None,
            local_runtime_role: None,
            canonical_hosted_guest_url: None,
            ephemeral_hosted_guest_url: None,
            browser_access_allowed: true,
            browser_access_block_reason: None,
            transport: RoomTransportView::default(),
        })
    })
}

pub fn load_room_control(data_dir: &Path) -> anyhow::Result<RoomControlSummary> {
    with_locked_state(data_dir, |_, state| {
        Ok(room_control_summary_from_state(state))
    })
}

pub fn local_runtime_access(
    data_dir: &Path,
    runtime_did: Option<&str>,
) -> anyhow::Result<LocalRuntimeAccess> {
    let normalized_did = match runtime_did {
        Some(did) if !did.trim().is_empty() => Some(normalize_member_did(did)?),
        _ => None,
    };
    with_locked_state(data_dir, |_, state| {
        Ok(local_runtime_access_from_state(
            state,
            normalized_did.as_deref(),
        ))
    })
}

pub fn seed_room_owner(
    data_dir: &Path,
    input: RoomOwnerSeedInput,
) -> anyhow::Result<RoomControlSummary> {
    let owner_did = normalize_member_did(&input.owner_did)?;
    let title = normalize_room_title(&input.title)?;
    with_locked_state(data_dir, |_, state| {
        let now = now_ts();
        let was_unseeded = state.control.owner_did.is_none();
        if let Some(existing_owner) = state.control.owner_did.as_deref() {
            if existing_owner != owner_did {
                anyhow::bail!("room owner already set to {}", existing_owner);
            }
        } else {
            state.control.owner_did = Some(owner_did.clone());
        }
        state.control.title = title.clone();
        if state.control.created_at == 0 {
            state.control.created_at = now;
        }
        ensure_active_member(
            state,
            &owner_did,
            RoomRole::Owner,
            &owner_did,
            state.control.created_at,
        );
        if was_unseeded
            && state.key_epochs.len() == 1
            && state.key_epochs[0].created_by == "unseeded-room"
            && state.key_epochs[0].epoch == state.control.current_key_epoch
        {
            state.key_epochs[0].created_by = owner_did.clone();
            state.key_epochs[0].created_at = state.control.created_at;
            state.key_epochs[0].reason = "initial room epoch".to_string();
        }
        state.control.updated_at = now;
        Ok(room_control_summary_from_state(state))
    })
}

pub fn update_room_access_policy(
    data_dir: &Path,
    input: RoomAccessPolicyUpdateInput,
) -> anyhow::Result<RoomAccessPolicyView> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    with_locked_state(data_dir, |_, state| {
        require_room_owner_seeded(state)?;
        let actor_role = require_active_member_role(state, &actor_did)?;
        if actor_role != RoomRole::Owner {
            anyhow::bail!("only the room owner can update room access policy");
        }

        state.control.allow_guest_invites = input.allow_guest_invites;
        state.control.allow_member_invites = input.allow_member_invites;
        state.control.allow_members_to_host_guests = input.allow_members_to_host_guests;
        state.control.updated_at = now_ts();
        Ok(access_policy_view_from_state(state))
    })
}

pub fn invite_room_member(
    data_dir: &Path,
    input: RoomInviteInput,
) -> anyhow::Result<RoomInviteView> {
    invite_room_member_with_profile(data_dir, input, None)
}

fn invite_room_member_with_profile(
    data_dir: &Path,
    input: RoomInviteInput,
    inviter_profile: Option<RoomProfileCardView>,
) -> anyhow::Result<RoomInviteView> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    let invited_did = normalize_member_did(&input.invited_did)?;
    if actor_did == invited_did {
        anyhow::bail!("actor DID must differ from invited DID");
    }
    with_locked_state(data_dir, |_, state| {
        require_room_owner_seeded(state)?;
        if !state.control.allow_member_invites {
            anyhow::bail!("ElastOS user invites are disabled for this conversation");
        }
        let actor_role = require_active_member_role(state, &actor_did)?;
        if !can_invite_role(&actor_role, &input.role) {
            anyhow::bail!(
                "{} cannot invite {:?}",
                actor_role_label(&actor_role),
                input.role
            );
        }
        if active_member_record(state, &invited_did).is_some() {
            anyhow::bail!("member is already active in the room");
        }
        if let Some(existing) = state.invites.iter().find(|invite| {
            invite.status == InviteStatus::Pending && invite.invited_did == invited_did
        }) {
            return Ok(room_invite_view_from_record(existing));
        }
        let now = now_ts();
        let invite = RoomInviteRecord {
            invite_id: random_hex(16),
            invited_did,
            role: input.role.clone(),
            invited_by: actor_did,
            inviter_profile,
            created_at: now,
            expires_at: now + INVITE_TTL_SECS,
            status: InviteStatus::Pending,
            acted_at: None,
            acted_by: None,
        };
        let out = room_invite_view_from_record(&invite);
        state.invites.push(invite);
        state.control.updated_at = now;
        Ok(out)
    })
}

pub fn accept_room_invite(
    data_dir: &Path,
    input: RoomInviteAcceptInput,
) -> anyhow::Result<RoomMemberView> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    with_locked_state(data_dir, |_, state| {
        let invite_index = state
            .invites
            .iter()
            .position(|invite| {
                invite.invite_id == input.invite_id
                    && invite.status == InviteStatus::Pending
                    && invite.invited_did == actor_did
            })
            .ok_or_else(|| anyhow::anyhow!("invite is not pending for this DID"))?;
        let now = now_ts();
        let role = state.invites[invite_index].role.clone();
        let invited_by = state.invites[invite_index].invited_by.clone();
        state.invites[invite_index].status = InviteStatus::Accepted;
        state.invites[invite_index].acted_at = Some(now);
        state.invites[invite_index].acted_by = Some(actor_did.clone());
        let member = ensure_active_member(state, &actor_did, role, &invited_by, now).clone();
        rotate_key_epoch_record(
            state,
            &actor_did,
            format!("membership changed: {} accepted room invite", actor_did),
            now,
        );
        Ok(room_member_view_from_record(&member))
    })
}

pub fn export_room_invite_envelope(
    data_dir: &Path,
    input: RoomInviteInput,
) -> anyhow::Result<SignedRoomInviteEnvelope> {
    export_room_invite_envelope_with_profile(data_dir, input, None)
}

pub fn export_room_invite_envelope_with_profile(
    data_dir: &Path,
    input: RoomInviteInput,
    inviter_profile: Option<RoomProfileCardView>,
) -> anyhow::Result<SignedRoomInviteEnvelope> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    let invite = invite_room_member_with_profile(data_dir, input, inviter_profile)?;
    let (signing_key, local_did) = elastos_identity::load_or_create_did(data_dir)?;
    let local_did = normalize_member_did(&local_did)?;
    if local_did != actor_did {
        anyhow::bail!(
            "local ElastOS identity {} does not match invite actor {}",
            local_did,
            actor_did
        );
    }

    let payload = with_locked_state(data_dir, |_, state| {
        require_active_member_role(state, &actor_did)?;
        let invite_record = state
            .invites
            .iter()
            .find(|record| record.invite_id == invite.invite_id)
            .ok_or_else(|| anyhow::anyhow!("invite disappeared before export"))?;
        Ok(SignedRoomInvitePayload {
            schema: ROOM_INVITE_ENVELOPE_SCHEMA.to_string(),
            room_slug: state.room_slug.clone(),
            room_title: state.control.title.clone(),
            owner_did: state.control.owner_did.clone(),
            current_key_epoch: state.control.current_key_epoch,
            invite_id: invite_record.invite_id.clone(),
            invited_did: invite_record.invited_did.clone(),
            role: invite_record.role.clone(),
            invited_by: actor_did.clone(),
            inviter_profile: invite_record.inviter_profile.clone(),
            created_at: invite_record.created_at,
            expires_at: invite_record.expires_at,
        })
    })?;
    let canonical = serde_json::to_string(&serde_json::to_value(&payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        ROOM_INVITE_ENVELOPE_DOMAIN,
        canonical.as_bytes(),
    );
    if signer_did != actor_did {
        anyhow::bail!(
            "signed invite envelope DID {} does not match actor {}",
            signer_did,
            actor_did
        );
    }

    Ok(SignedRoomInviteEnvelope {
        payload,
        signature,
        signer_did,
    })
}

pub fn export_room_join_invite(
    data_dir: &Path,
    input: RoomJoinInviteInput,
) -> anyhow::Result<RoomJoinInviteView> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    let issuer_gateway = normalize_join_invite_gateway(&input.issuer_gateway)?;
    let (signing_key, local_did) = elastos_identity::load_or_create_did(data_dir)?;
    let local_did = normalize_member_did(&local_did)?;
    if local_did != actor_did {
        anyhow::bail!(
            "local ElastOS identity {} does not match invite actor {}",
            local_did,
            actor_did
        );
    }

    let payload = with_locked_state(data_dir, |_, state| {
        require_room_owner_seeded(state)?;
        if !state.control.allow_member_invites {
            anyhow::bail!("ElastOS user invites are disabled for this conversation");
        }
        let actor_role = require_active_member_role(state, &actor_did)?;
        if !can_invite_role(&actor_role, &RoomRole::Member) {
            anyhow::bail!(
                "{} cannot create conversation join links",
                actor_role_label(&actor_role)
            );
        }
        let now = now_ts();
        Ok(SignedRoomJoinInvitePayload {
            schema: ROOM_JOIN_INVITE_SCHEMA.to_string(),
            room_slug: state.room_slug.clone(),
            room_title: state.control.title.clone(),
            issuer_gateway: issuer_gateway.clone(),
            invited_by: actor_did.clone(),
            inviter_profile: input.inviter_profile.clone(),
            role: RoomRole::Member,
            created_at: now,
            expires_at: now + INVITE_TTL_SECS,
        })
    })?;
    let canonical = serde_json::to_string(&serde_json::to_value(&payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        ROOM_JOIN_INVITE_DOMAIN,
        canonical.as_bytes(),
    );
    if signer_did != actor_did {
        anyhow::bail!(
            "signed join invite DID {} does not match actor {}",
            signer_did,
            actor_did
        );
    }
    let envelope = SignedRoomJoinInviteEnvelope {
        payload: payload.clone(),
        signature,
        signer_did,
    };
    let token = BASE64_URL_SAFE_NO_PAD.encode(serde_json::to_vec(&envelope)?);
    Ok(RoomJoinInviteView {
        invite_url: format!("elastos://peer/invite?token={token}"),
        token,
        issuer_gateway: payload.issuer_gateway,
        room_title: payload.room_title,
        invited_by: payload.invited_by,
        expires_at: payload.expires_at,
    })
}

pub fn claim_room_join_invite(
    data_dir: &Path,
    token_or_url: &str,
    invited_did: &str,
) -> anyhow::Result<SignedRoomInviteEnvelope> {
    let token = room_join_invite_token_from_input(token_or_url)?;
    let (envelope, signer_did) = decode_room_join_invite_token(&token)?;
    let payload = envelope.payload;
    if payload.invited_by != signer_did || envelope.signer_did != signer_did {
        anyhow::bail!("join invite signer does not match inviter DID");
    }
    export_room_invite_envelope_with_profile(
        data_dir,
        RoomInviteInput {
            actor_did: signer_did,
            invited_did: invited_did.to_string(),
            role: payload.role,
        },
        payload.inviter_profile,
    )
}

pub fn decode_room_join_invite_token(
    token: &str,
) -> anyhow::Result<(SignedRoomJoinInviteEnvelope, String)> {
    let token = room_join_invite_token_from_input(token)?;
    let bytes = BASE64_URL_SAFE_NO_PAD
        .decode(token.as_bytes())
        .context("conversation join token is not valid base64url")?;
    let (envelope_value, signer_did) = crate::crypto::verify_signed_json_envelope_against_dids(
        &bytes,
        ROOM_JOIN_INVITE_DOMAIN,
        &[],
    )?;
    let envelope: SignedRoomJoinInviteEnvelope = serde_json::from_value(envelope_value)?;
    let payload = &envelope.payload;
    if payload.schema != ROOM_JOIN_INVITE_SCHEMA {
        anyhow::bail!(
            "unsupported conversation join invite schema: {}",
            payload.schema
        );
    }
    if payload.room_slug != ROOM_SLUG {
        anyhow::bail!(
            "join invite is for room '{}' not '{}'",
            payload.room_slug,
            ROOM_SLUG
        );
    }
    if payload.expires_at <= now_ts() {
        anyhow::bail!("conversation join invite is expired");
    }
    let normalized_gateway = normalize_join_invite_gateway(&payload.issuer_gateway)?;
    if normalized_gateway != payload.issuer_gateway {
        anyhow::bail!("conversation join invite issuer gateway is not canonical");
    }
    if payload.invited_by != signer_did || envelope.signer_did != signer_did {
        anyhow::bail!("join invite signer does not match inviter DID");
    }
    Ok((envelope, signer_did))
}

pub fn room_join_invite_token_from_input(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("conversation join invite must not be empty");
    }
    if let Ok(parsed) = Url::parse(trimmed) {
        if parsed.scheme() == "elastos" {
            let path = parsed.path().trim_start_matches('/');
            if parsed.host_str() != Some("peer") || path != "invite" {
                anyhow::bail!("unsupported conversation join invite URI");
            }
            if let Some((_, token)) = parsed.query_pairs().find(|(key, _)| key == "token") {
                let token = token.trim().to_string();
                if token.is_empty() {
                    anyhow::bail!("conversation join invite link is missing a token");
                }
                return Ok(token);
            }
            anyhow::bail!("conversation join invite link is missing a token");
        }
    }
    Ok(trimmed.to_string())
}

pub fn import_room_invite_envelope(
    data_dir: &Path,
    envelope_bytes: &[u8],
) -> anyhow::Result<RoomInviteView> {
    let (envelope_value, signer_did) = crate::crypto::verify_signed_json_envelope_against_dids(
        envelope_bytes,
        ROOM_INVITE_ENVELOPE_DOMAIN,
        &[],
    )?;
    let envelope: SignedRoomInviteEnvelope = serde_json::from_value(envelope_value)?;
    let payload = envelope.payload;
    if payload.schema != ROOM_INVITE_ENVELOPE_SCHEMA {
        anyhow::bail!("unsupported room invite schema: {}", payload.schema);
    }
    if payload.room_slug != ROOM_SLUG {
        anyhow::bail!(
            "invite is for room '{}' not '{}'",
            payload.room_slug,
            ROOM_SLUG
        );
    }
    if payload.invited_by != signer_did || envelope.signer_did != signer_did {
        anyhow::bail!("invite signer does not match inviter DID");
    }
    let (_, local_did) = elastos_identity::load_or_create_did(data_dir)?;
    let local_did = normalize_member_did(&local_did)?;
    let invited_did = normalize_member_did(&payload.invited_did)?;
    if invited_did != local_did {
        anyhow::bail!(
            "invite is addressed to {} but local ElastOS identity is {}",
            invited_did,
            local_did
        );
    }
    if payload.expires_at <= now_ts() {
        anyhow::bail!("invite {} is already expired", payload.invite_id);
    }

    with_locked_state(data_dir, |_, state| {
        if let Some(existing) = state.invites.iter().find(|record| {
            record.invite_id == payload.invite_id
                && record.invited_did == invited_did
                && record.status == InviteStatus::Pending
        }) {
            return Ok(room_invite_view_from_record(existing));
        }
        if active_member_record(state, &invited_did).is_some() {
            anyhow::bail!("this device is already part of this conversation");
        }
        let room_was_uninitialized =
            state.control.owner_did.is_none() || state.control.created_at == 0;
        if let Some(owner_did) = payload.owner_did.clone() {
            let owner_did = normalize_member_did(&owner_did)?;
            match state.control.owner_did.as_deref() {
                Some(existing) if existing != owner_did => {
                    anyhow::bail!(
                        "room owner mismatch: local room expects {} but invite claims {}",
                        existing,
                        owner_did
                    );
                }
                None => state.control.owner_did = Some(owner_did.clone()),
                _ => {}
            }
            if owner_did != invited_did {
                ensure_active_member_with_profile(
                    state,
                    &owner_did,
                    RoomRole::Owner,
                    &owner_did,
                    payload.created_at,
                    payload.inviter_profile.clone(),
                );
            }
        }
        if room_was_uninitialized {
            state.control.title = payload.room_title.clone();
        }
        if state.control.created_at == 0 {
            state.control.created_at = payload.created_at;
        }
        state.control.updated_at = now_ts();
        if state.control.current_key_epoch < payload.current_key_epoch {
            state.control.current_key_epoch = payload.current_key_epoch;
        }

        let invite = RoomInviteRecord {
            invite_id: payload.invite_id.clone(),
            invited_did: invited_did.clone(),
            role: payload.role.clone(),
            invited_by: signer_did.clone(),
            inviter_profile: payload.inviter_profile.clone(),
            created_at: payload.created_at,
            expires_at: payload.expires_at,
            status: InviteStatus::Pending,
            acted_at: None,
            acted_by: None,
        };
        let out = room_invite_view_from_record(&invite);
        state.invites.push(invite);
        Ok(out)
    })
}

pub fn export_room_acceptance_envelope(
    data_dir: &Path,
    invite_id: &str,
) -> anyhow::Result<SignedRoomAcceptEnvelope> {
    export_room_acceptance_envelope_with_profile(data_dir, invite_id, None)
}

pub fn export_room_acceptance_envelope_with_profile(
    data_dir: &Path,
    invite_id: &str,
    member_profile: Option<RoomProfileCardView>,
) -> anyhow::Result<SignedRoomAcceptEnvelope> {
    let invite_id = invite_id.trim();
    if invite_id.is_empty() {
        anyhow::bail!("invite ID must not be empty");
    }

    let (signing_key, local_did) = elastos_identity::load_or_create_did(data_dir)?;
    let local_did = normalize_member_did(&local_did)?;
    let payload = with_locked_state(data_dir, |_, state| {
        let invite_record = state
            .invites
            .iter()
            .find(|record| {
                record.invite_id == invite_id
                    && record.invited_did == local_did
                    && record.status == InviteStatus::Accepted
            })
            .ok_or_else(|| anyhow::anyhow!("invite is not accepted by this runtime"))?;
        let accepted_at = invite_record.acted_at.unwrap_or_else(now_ts);
        Ok(SignedRoomAcceptPayload {
            schema: ROOM_ACCEPT_ENVELOPE_SCHEMA.to_string(),
            room_slug: state.room_slug.clone(),
            room_title: state.control.title.clone(),
            owner_did: state.control.owner_did.clone(),
            current_key_epoch: state.control.current_key_epoch,
            invite_id: invite_record.invite_id.clone(),
            member_did: local_did.clone(),
            role: invite_record.role.clone(),
            invited_by: invite_record.invited_by.clone(),
            member_profile: member_profile.clone(),
            accepted_at,
        })
    })?;
    let canonical = serde_json::to_string(&serde_json::to_value(&payload)?)?;
    let (signature, signer_did) = crate::crypto::domain_separated_sign(
        &signing_key,
        ROOM_ACCEPT_ENVELOPE_DOMAIN,
        canonical.as_bytes(),
    );
    if signer_did != local_did {
        anyhow::bail!(
            "signed acceptance envelope DID {} does not match local ElastOS identity {}",
            signer_did,
            local_did
        );
    }

    Ok(SignedRoomAcceptEnvelope {
        payload,
        signature,
        signer_did,
    })
}

pub fn import_room_acceptance_envelope(
    data_dir: &Path,
    envelope_bytes: &[u8],
) -> anyhow::Result<RoomMemberView> {
    let (envelope_value, signer_did) = crate::crypto::verify_signed_json_envelope_against_dids(
        envelope_bytes,
        ROOM_ACCEPT_ENVELOPE_DOMAIN,
        &[],
    )?;
    let envelope: SignedRoomAcceptEnvelope = serde_json::from_value(envelope_value)?;
    let payload = envelope.payload;
    if payload.schema != ROOM_ACCEPT_ENVELOPE_SCHEMA {
        anyhow::bail!("unsupported room acceptance schema: {}", payload.schema);
    }
    if payload.room_slug != ROOM_SLUG {
        anyhow::bail!(
            "acceptance is for room '{}' not '{}'",
            payload.room_slug,
            ROOM_SLUG
        );
    }
    let member_did = normalize_member_did(&payload.member_did)?;
    if member_did != signer_did || envelope.signer_did != signer_did {
        anyhow::bail!("acceptance signer does not match member DID");
    }

    with_locked_state(data_dir, |_, state| {
        if let Some(existing) = active_member_record(state, &member_did) {
            return Ok(room_member_view_from_record(existing));
        }
        let owner_did = payload
            .owner_did
            .clone()
            .map(|did| normalize_member_did(&did))
            .transpose()?;
        if let Some(ref owner_did) = owner_did {
            match state.control.owner_did.as_deref() {
                Some(existing) if existing != owner_did => {
                    anyhow::bail!(
                        "room owner mismatch: local room expects {} but acceptance claims {}",
                        existing,
                        owner_did
                    );
                }
                None => state.control.owner_did = Some(owner_did.clone()),
                _ => {}
            }
        }
        if state.control.title.trim().is_empty() {
            state.control.title = payload.room_title.clone();
        }
        if state.control.current_key_epoch < payload.current_key_epoch {
            state.control.current_key_epoch = payload.current_key_epoch;
        }
        let invite_index = state
            .invites
            .iter()
            .position(|invite| {
                invite.invite_id == payload.invite_id
                    && invite.status == InviteStatus::Pending
                    && invite.invited_did == member_did
            })
            .ok_or_else(|| anyhow::anyhow!("pending invite not found for acceptance envelope"))?;
        let invite = &state.invites[invite_index];
        if invite.role != payload.role {
            anyhow::bail!(
                "invite role mismatch: local invite is {} but acceptance claims {}",
                role_label(&invite.role),
                role_label(&payload.role)
            );
        }
        if invite.invited_by != payload.invited_by {
            anyhow::bail!(
                "inviter mismatch: local invite is from {} but acceptance claims {}",
                invite.invited_by,
                payload.invited_by
            );
        }

        state.invites[invite_index].status = InviteStatus::Accepted;
        state.invites[invite_index].acted_at = Some(payload.accepted_at);
        state.invites[invite_index].acted_by = Some(member_did.clone());
        let member = ensure_active_member_with_profile(
            state,
            &member_did,
            payload.role.clone(),
            &payload.invited_by,
            payload.accepted_at,
            payload.member_profile.clone(),
        )
        .clone();
        rotate_key_epoch_record(
            state,
            &member_did,
            format!("membership changed: {} accepted room invite", member_did),
            payload.accepted_at,
        );
        Ok(room_member_view_from_record(&member))
    })
}

pub fn revoke_room_invite(
    data_dir: &Path,
    actor_did: &str,
    invite_id: &str,
) -> anyhow::Result<Option<RoomInviteView>> {
    let actor_did = normalize_member_did(actor_did)?;
    with_locked_state(data_dir, |_, state| {
        require_room_owner_seeded(state)?;
        let actor_role = require_active_member_role(state, &actor_did)?;
        let Some(invite_index) = state.invites.iter().position(|invite| {
            invite.invite_id == invite_id && invite.status == InviteStatus::Pending
        }) else {
            return Ok(None);
        };
        let invite = &state.invites[invite_index];
        if !can_invite_role(&actor_role, &invite.role) {
            anyhow::bail!(
                "{} cannot revoke {:?} invites",
                actor_role_label(&actor_role),
                invite.role
            );
        }

        let now = now_ts();
        state.invites[invite_index].status = InviteStatus::Revoked;
        state.invites[invite_index].acted_at = Some(now);
        state.invites[invite_index].acted_by = Some(actor_did);
        state.control.updated_at = now;
        Ok(Some(room_invite_view_from_record(
            &state.invites[invite_index],
        )))
    })
}

pub fn remove_room_member(
    data_dir: &Path,
    input: RoomMemberRemoveInput,
) -> anyhow::Result<Option<RoomMemberView>> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    let member_did = normalize_member_did(&input.member_did)?;
    with_locked_state(data_dir, |_, state| {
        let actor_role = require_active_member_role(state, &actor_did)?;
        let member = match active_member_record_mut(state, &member_did) {
            Some(member) => member,
            None => return Ok(None),
        };
        if member.role == RoomRole::Owner {
            anyhow::bail!("owner cannot be removed");
        }
        if !can_remove_role(&actor_role, &member.role) {
            anyhow::bail!(
                "{} cannot remove {}",
                actor_role_label(&actor_role),
                role_label(&member.role)
            );
        }
        member.active = false;
        member.removed_at = Some(now_ts());
        member.removed_by = Some(actor_did.clone());
        let out = room_member_view_from_record(member);
        rotate_key_epoch_record(
            state,
            &actor_did,
            format!("membership changed: {} removed from room", member_did),
            now_ts(),
        );
        Ok(Some(out))
    })
}

pub fn rotate_room_key_epoch(
    data_dir: &Path,
    input: RoomKeyRotateInput,
) -> anyhow::Result<RoomKeyEpochView> {
    let actor_did = normalize_member_did(&input.actor_did)?;
    let reason = normalize_key_rotation_reason(&input.reason)?;
    with_locked_state(data_dir, |_, state| {
        let actor_role = require_active_member_role(state, &actor_did)?;
        if actor_role != RoomRole::Owner {
            anyhow::bail!("only the room owner can rotate room keys directly");
        }
        Ok(room_key_epoch_view_from_record(rotate_key_epoch_record(
            state,
            &actor_did,
            reason,
            now_ts(),
        )))
    })
}

pub fn reset_room(data_dir: &Path) -> anyhow::Result<RoomResetOutput> {
    with_locked_state(data_dir, |paths, state| {
        let cleared_requests = state.pending_requests.len();
        let cleared_sessions = state.sessions.len();
        let cleared_objects = state.objects.len();
        let cleared_uploads = state.uploads.len();
        let cleared_attachments = count_dir_entries(&paths.attachments_dir)?;

        state.pending_requests.clear();
        state.sessions.clear();
        state.objects.clear();
        state.uploads.clear();
        state.next_seq = 1;
        state.control.updated_at = now_ts();

        remove_dir_all_if_exists(&paths.attachments_dir)?;
        remove_dir_all_if_exists(&paths.uploads_dir)?;

        Ok(RoomResetOutput {
            room_slug: state.room_slug.clone(),
            cleared_requests,
            cleared_sessions,
            cleared_objects,
            cleared_uploads,
            cleared_attachments,
        })
    })
}

pub fn request_browser_access(
    data_dir: &Path,
    input: BrowserAccessRequestInput,
) -> anyhow::Result<BrowserAccessRequestOutput> {
    let display_name = normalize_display_name(&input.display_name)?;
    let device_label = normalize_device_label(&input.device_label);
    let capabilities = normalize_browser_session_capabilities(&input.capabilities)?;
    let host_member_did = input
        .host_member_did
        .as_deref()
        .filter(|did| !did.trim().is_empty())
        .map(normalize_member_did)
        .transpose()?;

    with_locked_state(data_dir, |_, state| {
        ensure_browser_access_allowed_for_member(state, host_member_did.as_deref())?;
        let now = now_ts();
        let request = BrowserAccessRequestRecord {
            request_id: random_hex(16),
            display_name,
            device_label,
            host_member_did,
            capabilities: capabilities.clone(),
            requested_at: now,
            expires_at: now + BROWSER_ACCESS_REQUEST_TTL_SECS,
            status: BrowserAccessStatus::Pending,
            denial_reason: None,
            session_token: None,
            session_expires_at: None,
        };
        let out = BrowserAccessRequestOutput {
            request_id: request.request_id.clone(),
            room_slug: state.room_slug.clone(),
            status: "pending".to_string(),
            requested_at: request.requested_at,
            capabilities,
        };
        state.pending_requests.push(request);
        Ok(out)
    })
}

pub fn browser_access_status(
    data_dir: &Path,
    request_id: &str,
) -> anyhow::Result<BrowserAccessStatusOutput> {
    with_locked_state(data_dir, |_, state| {
        let request = state
            .pending_requests
            .iter()
            .find(|item| item.request_id == request_id)
            .ok_or_else(|| anyhow::anyhow!("web guest request not found"))?;

        Ok(BrowserAccessStatusOutput {
            request_id: request.request_id.clone(),
            room_slug: state.room_slug.clone(),
            status: browser_access_status_label(&request.status).to_string(),
            token: request.session_token.clone(),
            expires_at: request.session_expires_at,
            denial_reason: request.denial_reason.clone(),
            capabilities: request.capabilities.clone(),
        })
    })
}

pub fn start_local_runtime_session(
    data_dir: &Path,
    member_did: &str,
    display_name: &str,
    device_label: &str,
) -> anyhow::Result<LocalRuntimeSessionOutput> {
    Ok(start_local_runtime_session_with_transport(
        data_dir,
        member_did,
        display_name,
        device_label,
    )?
    .session)
}

pub fn start_local_runtime_session_with_transport(
    data_dir: &Path,
    member_did: &str,
    display_name: &str,
    device_label: &str,
) -> anyhow::Result<LocalRuntimeSessionWithTransportOutput> {
    start_local_runtime_session_for_actor(data_dir, member_did, None, display_name, device_label)
}

pub fn start_local_principal_runtime_session(
    data_dir: &Path,
    member_did: &str,
    principal_id: &str,
    display_name: &str,
    device_label: &str,
) -> anyhow::Result<LocalRuntimeSessionOutput> {
    let actor_id = local_principal_room_actor_id(principal_id)?;
    Ok(start_local_runtime_session_for_actor(
        data_dir,
        member_did,
        Some(actor_id),
        display_name,
        device_label,
    )?
    .session)
}

pub fn start_local_principal_runtime_session_with_transport(
    data_dir: &Path,
    member_did: &str,
    principal_id: &str,
    display_name: &str,
    device_label: &str,
) -> anyhow::Result<LocalRuntimeSessionWithTransportOutput> {
    let actor_id = local_principal_room_actor_id(principal_id)?;
    start_local_runtime_session_for_actor(
        data_dir,
        member_did,
        Some(actor_id),
        display_name,
        device_label,
    )
}

fn start_local_runtime_session_for_actor(
    data_dir: &Path,
    member_did: &str,
    actor_id: Option<String>,
    display_name: &str,
    device_label: &str,
) -> anyhow::Result<LocalRuntimeSessionWithTransportOutput> {
    let member_did = normalize_member_did(member_did)?;
    let display_name = normalize_display_name(display_name)?;
    let device_label = normalize_device_label(device_label);

    with_locked_state(data_dir, |paths, state| {
        let local_access = local_runtime_access_from_state(state, Some(&member_did));
        if local_access.member_role.is_none() && !local_access.browser_access_allowed {
            anyhow::bail!(
                "{}",
                local_access.block_reason.unwrap_or_else(|| {
                    "this device is not part of this conversation".to_string()
                })
            );
        }

        let session_member_did = Some(member_did.clone());
        let capabilities = room_access_capabilities();

        if actor_id
            .as_deref()
            .is_some_and(|actor| actor.starts_with("principal:"))
        {
            state.sessions.retain(|session| {
                session.member_did.as_deref() != Some(member_did.as_str())
                    || session.actor_id.trim().starts_with("principal:")
            });
        }

        if let Some(session_member_did) = session_member_did.as_deref() {
            if active_member_record(state, session_member_did).is_none()
                && room_requires_active_membership(state)
            {
                anyhow::bail!("this device is not part of this conversation");
            }
        }
        let now = now_ts();
        let canonical_local_token = state
            .sessions
            .iter()
            .filter(|session| match actor_id.as_deref() {
                Some(actor_id) => session.actor_id == actor_id,
                None => {
                    session.member_did.as_deref() == Some(member_did.as_str())
                        || (session.member_did.is_none() && session.device_label == device_label)
                }
            })
            .max_by_key(|session| {
                (
                    session.member_did.is_some(),
                    session.last_seen_at,
                    session.expires_at,
                )
            })
            .map(|session| session.token.clone());

        if let Some(token) = canonical_local_token {
            state.sessions.retain(|session| match actor_id.as_deref() {
                Some(actor_id) => session.actor_id != actor_id || session.token == token,
                None => {
                    !(session.member_did.as_deref() == Some(member_did.as_str())
                        || (session.member_did.is_none() && session.device_label == device_label))
                        || session.token == token
                }
            });
            let existing = state
                .sessions
                .iter_mut()
                .find(|session| session.token == token)
                .expect("canonical local runtime session");
            if let Some(actor_id) = actor_id.as_deref() {
                existing.actor_id = actor_id.to_string();
            }
            existing.display_name = display_name.clone();
            existing.device_label = device_label.clone();
            existing.member_did = session_member_did.clone();
            existing.capabilities = capabilities.clone();
            existing.last_seen_at = now;
            existing.expires_at = now + SESSION_TTL_SECS;
            return Ok(LocalRuntimeSessionWithTransportOutput {
                session: LocalRuntimeSessionOutput {
                    token: existing.token.clone(),
                    display_name: existing.display_name.clone(),
                    expires_at: existing.expires_at,
                    capabilities: existing.capabilities.clone(),
                },
                transport_envelope: None,
            });
        }

        let had_existing_session = match (actor_id.as_deref(), session_member_did.as_deref()) {
            (Some(actor_id), _) => state
                .sessions
                .iter()
                .any(|session| session.actor_id == actor_id),
            (None, Some(did)) => active_local_session_count(state, did) > 0,
            _ => false,
        };
        let session = create_session_record_with_actor(
            &display_name,
            &device_label,
            session_member_did.clone(),
            actor_id.clone(),
            capabilities.clone(),
            now,
        );
        let output = LocalRuntimeSessionOutput {
            token: session.token.clone(),
            display_name: session.display_name.clone(),
            expires_at: session.expires_at,
            capabilities,
        };
        state.sessions.push(session);
        let mut transport_envelope = None;
        match session_member_did.as_deref() {
            Some(member_did) if !had_existing_session => {
                let object = push_member_system_object_for_actor(
                    state,
                    member_did,
                    actor_id.as_deref(),
                    display_name,
                    "joined the room".to_string(),
                    now,
                );
                transport_envelope = transport_envelope_from_record(paths, state, &object);
            }
            None => {
                push_system_object(state, display_name, "joined the room".to_string(), now);
            }
            _ => {}
        }
        Ok(LocalRuntimeSessionWithTransportOutput {
            session: output,
            transport_envelope,
        })
    })
}

fn local_principal_room_actor_id(principal_id: &str) -> anyhow::Result<String> {
    let value = principal_id.trim();
    if value.is_empty() || value.chars().count() > 240 {
        anyhow::bail!("principal id is invalid for room actor binding");
    }
    let digest = sha2::Sha256::digest(format!("elastos.room.actor.v1:{value}").as_bytes());
    Ok(format!("principal:{}", hex::encode(&digest[..16])))
}

pub fn approve_next_request(data_dir: &Path) -> anyhow::Result<Option<ApprovalOutcome>> {
    with_locked_state(data_dir, |_, state| {
        let Some(request_index) = state
            .pending_requests
            .iter()
            .position(|item| item.status == BrowserAccessStatus::Pending)
        else {
            return Ok(None);
        };

        Ok(Some(approve_request_at_index(state, request_index)?))
    })
}

pub fn approve_request(
    data_dir: &Path,
    request_id: &str,
) -> anyhow::Result<Option<ApprovalOutcome>> {
    with_locked_state(data_dir, |_, state| {
        let Some(request_index) = state.pending_requests.iter().position(|item| {
            item.status == BrowserAccessStatus::Pending && item.request_id == request_id
        }) else {
            return Ok(None);
        };

        Ok(Some(approve_request_at_index(state, request_index)?))
    })
}

fn approve_request_at_index(
    state: &mut RoomState,
    request_index: usize,
) -> anyhow::Result<ApprovalOutcome> {
    let now = now_ts();
    let host_member_did = state.pending_requests[request_index]
        .host_member_did
        .clone();
    if room_requires_active_membership(state) {
        let member_did = host_member_did.as_deref().ok_or_else(|| {
            anyhow::anyhow!("web guest request is not bound to a host ElastOS identity")
        })?;
        if active_member_record(state, member_did).is_none() {
            anyhow::bail!("web guest request host is no longer part of this conversation");
        }
    }
    let (request_id, display_name, device_label, capabilities) = {
        let request = &mut state.pending_requests[request_index];
        let request_id = request.request_id.clone();
        let display_name = request.display_name.clone();
        let device_label = request.device_label.clone();
        let capabilities = request.capabilities.clone();
        request.status = BrowserAccessStatus::Approved;
        request.denial_reason = None;
        (request_id, display_name, device_label, capabilities)
    };
    let session = create_session_record(&display_name, &device_label, None, capabilities, now);
    let expires_at = session.expires_at;
    {
        let request = &mut state.pending_requests[request_index];
        request.session_token = Some(session.token.clone());
        request.session_expires_at = Some(expires_at);
    }

    state.sessions.push(session);
    push_system_object(
        state,
        display_name.clone(),
        "joined the room".to_string(),
        now,
    );

    Ok(ApprovalOutcome {
        request_id,
        display_name,
        device_label,
        expires_at,
    })
}

fn create_session_record(
    display_name: &str,
    device_label: &str,
    member_did: Option<String>,
    capabilities: Vec<String>,
    now: u64,
) -> SessionRecord {
    create_session_record_with_actor(
        display_name,
        device_label,
        member_did,
        None,
        capabilities,
        now,
    )
}

fn create_session_record_with_actor(
    display_name: &str,
    device_label: &str,
    member_did: Option<String>,
    actor_id: Option<String>,
    capabilities: Vec<String>,
    now: u64,
) -> SessionRecord {
    SessionRecord {
        token: random_hex(32),
        actor_id: actor_id.unwrap_or_else(|| random_hex(16)),
        display_name: display_name.to_string(),
        device_label: device_label.to_string(),
        member_did,
        capabilities,
        approved_at: now,
        expires_at: now + SESSION_TTL_SECS,
        last_seen_at: now,
    }
}

fn ensure_session_actor_id(session: &mut SessionRecord) {
    if session.actor_id.trim().is_empty() {
        session.actor_id = random_hex(16);
    }
}

fn current_session_identity(session: &SessionRecord) -> CurrentSessionIdentity {
    CurrentSessionIdentity {
        actor_id: session.actor_id.clone(),
        member_did: session.member_did.clone(),
    }
}

pub fn deny_next_request(data_dir: &Path, reason: &str) -> anyhow::Result<Option<DenyOutcome>> {
    let reason = normalize_denial_reason(reason);
    with_locked_state(data_dir, |_, state| {
        let Some(request_index) = state
            .pending_requests
            .iter()
            .position(|item| item.status == BrowserAccessStatus::Pending)
        else {
            return Ok(None);
        };

        Ok(Some(deny_request_at_index(state, request_index, reason)))
    })
}

pub fn deny_request(
    data_dir: &Path,
    request_id: &str,
    reason: &str,
) -> anyhow::Result<Option<DenyOutcome>> {
    let reason = normalize_denial_reason(reason);
    with_locked_state(data_dir, |_, state| {
        let Some(request_index) = state.pending_requests.iter().position(|item| {
            item.status == BrowserAccessStatus::Pending && item.request_id == request_id
        }) else {
            return Ok(None);
        };

        Ok(Some(deny_request_at_index(state, request_index, reason)))
    })
}

fn deny_request_at_index(
    state: &mut RoomState,
    request_index: usize,
    reason: String,
) -> DenyOutcome {
    let request = &mut state.pending_requests[request_index];
    request.status = BrowserAccessStatus::Denied;
    request.denial_reason = Some(reason.clone());
    request.session_token = None;
    request.session_expires_at = None;

    DenyOutcome {
        request_id: request.request_id.clone(),
        display_name: request.display_name.clone(),
        device_label: request.device_label.clone(),
        reason,
    }
}

pub fn revoke_all_sessions(data_dir: &Path) -> anyhow::Result<Option<RevokeOutcome>> {
    with_locked_state(data_dir, |paths, state| {
        if state.sessions.is_empty() {
            return Ok(None);
        }

        let now = now_ts();
        let revoked_sessions = std::mem::take(&mut state.sessions);
        let revoked_tokens = revoked_sessions
            .iter()
            .map(|session| session.token.clone())
            .collect::<Vec<_>>();
        let mut removed_member_presence = BTreeSet::new();
        invalidate_approved_requests(state, &revoked_tokens);
        remove_uploads_for_tokens(paths, state, &revoked_tokens);
        let revoked_participants = revoked_sessions
            .into_iter()
            .map(|session| {
                let participant = ParticipantView {
                    display_name: session.display_name.clone(),
                    device_label: session.device_label.clone(),
                    last_seen_at: session.last_seen_at,
                    member_did: session.member_did.clone(),
                    role: session
                        .member_did
                        .as_deref()
                        .and_then(|did| active_member_record(state, did))
                        .map(|member| member.role.clone()),
                    local_session_count: 1,
                    is_current_session: false,
                };
                match session.member_did.as_deref() {
                    Some(member_did) => {
                        if removed_member_presence.insert(member_did.to_string())
                            && !state
                                .sessions
                                .iter()
                                .any(|active| active.member_did.as_deref() == Some(member_did))
                        {
                            push_member_system_object(
                                state,
                                member_did,
                                session.display_name,
                                "was removed from the room in Home".to_string(),
                                now,
                            );
                        }
                    }
                    None => {
                        push_system_object(
                            state,
                            session.display_name,
                            "was removed from the room in Home".to_string(),
                            now,
                        );
                    }
                }
                participant
            })
            .collect::<Vec<_>>();

        Ok(Some(RevokeOutcome {
            revoked_count: revoked_participants.len(),
            revoked_participants,
        }))
    })
}

pub fn revoke_session(
    data_dir: &Path,
    token: &str,
) -> anyhow::Result<Option<RevokeSessionOutcome>> {
    with_locked_state(data_dir, |paths, state| {
        let Some(session_index) = state
            .sessions
            .iter()
            .position(|session| session.token == token)
        else {
            return Ok(None);
        };

        let now = now_ts();
        let session = state.sessions.remove(session_index);
        invalidate_approved_requests(state, std::slice::from_ref(&session.token));
        remove_uploads_for_tokens(paths, state, std::slice::from_ref(&session.token));
        match session.member_did.as_deref() {
            Some(member_did)
                if !state
                    .sessions
                    .iter()
                    .any(|active| active.member_did.as_deref() == Some(member_did)) =>
            {
                push_member_system_object(
                    state,
                    member_did,
                    session.display_name.clone(),
                    "was removed from the room in Home".to_string(),
                    now,
                );
            }
            None => {
                push_system_object(
                    state,
                    session.display_name.clone(),
                    "was removed from the room in Home".to_string(),
                    now,
                );
            }
            _ => {}
        }
        Ok(Some(RevokeSessionOutcome {
            token: session.token,
            display_name: session.display_name,
            device_label: session.device_label,
        }))
    })
}

pub fn revoke_guest_session_by_id(
    data_dir: &Path,
    session_id: &str,
) -> anyhow::Result<Option<RevokeSessionOutcome>> {
    let session_id = normalize_session_id(session_id)?;
    with_locked_state(data_dir, |paths, state| {
        let Some(session_index) = state
            .sessions
            .iter()
            .position(|session| session_public_id(&session.token) == session_id)
        else {
            return Ok(None);
        };
        if state.sessions[session_index].member_did.is_some() {
            anyhow::bail!("runtime node sessions must be blocked by removing the member DID");
        }

        let now = now_ts();
        let session = state.sessions.remove(session_index);
        invalidate_approved_requests(state, std::slice::from_ref(&session.token));
        remove_uploads_for_tokens(paths, state, std::slice::from_ref(&session.token));
        push_system_object(
            state,
            session.display_name.clone(),
            "was removed from the room in Home".to_string(),
            now,
        );
        Ok(Some(RevokeSessionOutcome {
            token: session.token,
            display_name: session.display_name,
            device_label: session.device_label,
        }))
    })
}

pub fn session_view(data_dir: &Path, token: &str) -> anyhow::Result<SessionView> {
    with_locked_state(data_dir, |_, state| {
        let room_slug = state.room_slug.clone();
        let (display_name, expires_at, capabilities, current_session) = {
            let session = validate_session(state, token)?;
            (
                session.display_name.clone(),
                session.expires_at,
                session.capabilities.clone(),
                current_session_identity(session),
            )
        };
        let participants = participant_views_from_state(state, Some(&current_session));
        Ok(SessionView {
            room_slug,
            display_name,
            expires_at,
            latest_seq: state.objects.last().map(|item| item.seq).unwrap_or(0),
            capabilities,
            participants,
        })
    })
}

pub fn leave_session(data_dir: &Path, token: &str) -> anyhow::Result<ConversationObjectView> {
    Ok(leave_session_with_transport(data_dir, token)?.object)
}

pub fn leave_session_with_transport(
    data_dir: &Path,
    token: &str,
) -> anyhow::Result<AppendedConversationObject> {
    with_locked_state(data_dir, |paths, state| {
        let now = now_ts();
        let session_index = state
            .sessions
            .iter()
            .position(|session| session.token == token)
            .ok_or_else(|| anyhow::anyhow!("invalid or expired session"))?;
        let session = state.sessions.remove(session_index);
        invalidate_approved_requests(state, std::slice::from_ref(&session.token));
        remove_uploads_for_tokens(paths, state, std::slice::from_ref(&session.token));
        let sender_member_did = session.member_did.clone();
        let object = match session.member_did.as_deref() {
            Some(member_did)
                if !state
                    .sessions
                    .iter()
                    .any(|active| active.member_did.as_deref() == Some(member_did)) =>
            {
                push_member_system_object(
                    state,
                    member_did,
                    session.display_name,
                    "left the room".to_string(),
                    now,
                )
            }
            _ => push_system_object(
                state,
                session.display_name,
                "left the room".to_string(),
                now,
            ),
        };
        let transport_envelope = transport_envelope_from_record(paths, state, &object);
        Ok(AppendedConversationObject {
            object: object_view_from_record(object, None),
            sender_member_did,
            transport_envelope,
        })
    })
}

pub fn conversation_feed(
    data_dir: &Path,
    token: &str,
    since: u64,
) -> anyhow::Result<ConversationFeed> {
    with_locked_state(data_dir, |_, state| {
        let current_session = {
            let session = validate_session(state, token)?;
            current_session_identity(session)
        };
        let objects = state
            .objects
            .iter()
            .filter(|item| item.seq > since)
            .cloned()
            .map(|item| object_view_from_record(item, Some(&current_session)))
            .collect::<Vec<_>>();
        Ok(ConversationFeed {
            room_slug: state.room_slug.clone(),
            latest_seq: state.objects.last().map(|item| item.seq).unwrap_or(0),
            objects,
        })
    })
}

pub fn room_poll(data_dir: &Path, token: &str, since: u64) -> anyhow::Result<RoomPollView> {
    with_locked_state(data_dir, |_, state| {
        let room_slug = state.room_slug.clone();
        let (display_name, expires_at, current_session) = {
            let session = validate_session(state, token)?;
            session.last_seen_at = now_ts();
            (
                session.display_name.clone(),
                session.expires_at,
                current_session_identity(session),
            )
        };
        let participants = participant_views_from_state(state, Some(&current_session));
        let objects = state
            .objects
            .iter()
            .filter(|item| item.seq > since)
            .cloned()
            .map(|item| object_view_from_record(item, Some(&current_session)))
            .collect::<Vec<_>>();
        Ok(RoomPollView {
            room_slug,
            display_name,
            expires_at,
            latest_seq: state.objects.last().map(|item| item.seq).unwrap_or(0),
            participants,
            objects,
            transport: RoomTransportView::default(),
        })
    })
}

pub fn append_object(
    data_dir: &Path,
    token: &str,
    body: &str,
) -> anyhow::Result<ConversationObjectView> {
    Ok(append_object_with_transport(data_dir, token, body)?.object)
}

pub fn append_object_with_transport(
    data_dir: &Path,
    token: &str,
    body: &str,
) -> anyhow::Result<AppendedConversationObject> {
    let draft = classify_object_body(body)?;
    with_locked_state(data_dir, |paths, state| {
        let (sender, sender_member_did, sender_actor_id, current_session) = {
            let session = validate_session(state, token)?;
            session.last_seen_at = now_ts();
            (
                session.display_name.clone(),
                session.member_did.clone(),
                session.actor_id.clone(),
                current_session_identity(session),
            )
        };
        let created_at = now_ts();
        let object = push_object(
            state,
            ConversationObjectRecord {
                seq: 0,
                event_id: new_object_event_id(),
                sender,
                sender_member_did: sender_member_did.clone(),
                sender_actor_id,
                kind: draft.kind,
                body: draft.body,
                emoji: draft.emoji,
                link: draft.link,
                attachment: None,
                created_at,
            },
        );
        let transport_envelope = transport_envelope_from_record(paths, state, &object);
        Ok(AppendedConversationObject {
            object: object_view_from_record(object, Some(&current_session)),
            sender_member_did,
            transport_envelope,
        })
    })
}

pub fn ingest_room_object_envelope(
    data_dir: &Path,
    envelope: &RoomObjectEnvelope,
) -> anyhow::Result<Option<ConversationObjectView>> {
    validate_room_object_envelope(envelope)?;
    let sender_member_did = normalize_member_did(&envelope.sender_member_did)?;
    let event_id = normalize_room_object_event_id(&envelope.event_id)?;
    with_locked_state(data_dir, |paths, state| {
        if envelope.room_slug != state.room_slug {
            anyhow::bail!(
                "room object envelope is for room '{}' not '{}'",
                envelope.room_slug,
                state.room_slug
            );
        }
        if active_member_record(state, &sender_member_did).is_none()
            && room_requires_active_membership(state)
        {
            anyhow::bail!("sender member DID is not active in this room");
        }
        if state
            .objects
            .iter()
            .any(|object| object.event_id == event_id)
        {
            return Ok(None);
        }
        let ConversationObjectDraft {
            kind,
            body,
            emoji,
            link,
            attachment,
            attachment_bytes,
        } = normalize_transport_object_payload(envelope)?;
        if let (Some(attachment), Some(bytes)) = (attachment.as_ref(), attachment_bytes.as_deref())
        {
            write_bytes_atomic(
                &paths.attachments_dir.join(&attachment.attachment_id),
                bytes,
            )?;
        }
        let object = push_object(
            state,
            ConversationObjectRecord {
                seq: 0,
                event_id,
                sender: normalize_display_name(&envelope.sender)?,
                sender_member_did: Some(sender_member_did),
                sender_actor_id: String::new(),
                kind,
                body,
                emoji,
                link,
                attachment,
                created_at: envelope.created_at,
            },
        );
        Ok(Some(object_view_from_record(object, None)))
    })
}

pub fn recent_local_room_object_envelopes(
    data_dir: &Path,
    sender_member_did: &str,
    limit: usize,
) -> anyhow::Result<Vec<RoomObjectEnvelope>> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let sender_member_did = normalize_member_did(sender_member_did)?;
    with_locked_state(data_dir, |paths, state| {
        let mut envelopes = state
            .objects
            .iter()
            .rev()
            .filter(|object| {
                object
                    .sender_member_did
                    .as_deref()
                    .map(|did| did == sender_member_did)
                    .unwrap_or(false)
            })
            .filter_map(|object| transport_envelope_from_record(paths, state, object))
            .take(limit)
            .collect::<Vec<_>>();
        envelopes.reverse();
        Ok(envelopes)
    })
}

pub fn append_attachment_object(
    data_dir: &Path,
    token: &str,
    file_name: &str,
    mime_type: &str,
    bytes: &[u8],
) -> anyhow::Result<ConversationObjectView> {
    Ok(
        append_attachment_object_with_transport(data_dir, token, file_name, mime_type, bytes)?
            .object,
    )
}

pub fn append_attachment_object_with_transport(
    data_dir: &Path,
    token: &str,
    file_name: &str,
    mime_type: &str,
    bytes: &[u8],
) -> anyhow::Result<AppendedConversationObject> {
    let file_name = normalize_file_name(file_name);
    let mime_type = normalize_mime_type(mime_type);
    if bytes.is_empty() {
        anyhow::bail!("attachment must not be empty");
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        anyhow::bail!("attachment exceeds {} bytes", MAX_ATTACHMENT_BYTES);
    }

    with_locked_state(data_dir, |paths, state| {
        let (sender, sender_member_did, sender_actor_id, current_session) = {
            let session = validate_session(state, token)?;
            session.last_seen_at = now_ts();
            (
                session.display_name.clone(),
                session.member_did.clone(),
                session.actor_id.clone(),
                current_session_identity(session),
            )
        };
        let created_at = now_ts();
        let object = append_attachment_record(
            paths,
            state,
            AttachmentRecordInput {
                sender,
                sender_member_did: sender_member_did.clone(),
                sender_actor_id,
                created_at,
                file_name: &file_name,
                mime_type: &mime_type,
                bytes,
            },
        )?;
        let transport_envelope = transport_envelope_from_record(paths, state, &object);
        Ok(AppendedConversationObject {
            object: object_view_from_record(object, Some(&current_session)),
            sender_member_did,
            transport_envelope,
        })
    })
}

pub fn start_attachment_upload(
    data_dir: &Path,
    token: &str,
    file_name: &str,
    mime_type: &str,
    size_bytes: u64,
) -> anyhow::Result<AttachmentUploadStartOutput> {
    let file_name = normalize_file_name(file_name);
    let mime_type = normalize_mime_type(mime_type);
    if size_bytes == 0 {
        anyhow::bail!("attachment must not be empty");
    }
    if size_bytes > MAX_ATTACHMENT_BYTES as u64 {
        anyhow::bail!("attachment exceeds {} bytes", MAX_ATTACHMENT_BYTES);
    }

    with_locked_state(data_dir, |paths, state| {
        let session = validate_session(state, token)?;
        session.last_seen_at = now_ts();

        let now = now_ts();
        let upload_id = random_hex(16);
        fs::create_dir_all(&paths.uploads_dir)?;
        fs::write(upload_staging_path(paths, &upload_id), [])?;
        state.uploads.push(UploadRecord {
            upload_id: upload_id.clone(),
            token: token.to_string(),
            file_name,
            mime_type,
            size_bytes,
            received_bytes: 0,
            created_at: now,
            expires_at: now + UPLOAD_TTL_SECS,
        });
        Ok(AttachmentUploadStartOutput {
            upload_id,
            chunk_size_bytes: ATTACHMENT_UPLOAD_CHUNK_BYTES,
            received_bytes: 0,
            expires_at: now + UPLOAD_TTL_SECS,
        })
    })
}

pub fn append_attachment_upload_chunk(
    data_dir: &Path,
    token: &str,
    upload_id: &str,
    offset: u64,
    bytes: &[u8],
) -> anyhow::Result<AttachmentUploadChunkOutput> {
    if bytes.is_empty() {
        anyhow::bail!("upload chunk must not be empty");
    }
    if bytes.len() > ATTACHMENT_UPLOAD_CHUNK_BYTES {
        anyhow::bail!(
            "upload chunk exceeds {} bytes",
            ATTACHMENT_UPLOAD_CHUNK_BYTES
        );
    }

    with_locked_state(data_dir, |paths, state| {
        let session = validate_session(state, token)?;
        session.last_seen_at = now_ts();

        let upload = find_upload_mut(state, token, upload_id)?;
        if upload.received_bytes != offset {
            anyhow::bail!(
                "upload offset mismatch: expected {}, got {}",
                upload.received_bytes,
                offset
            );
        }
        let next_size = upload
            .received_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| anyhow::anyhow!("upload exceeds supported size"))?;
        if next_size > upload.size_bytes {
            anyhow::bail!("upload exceeds declared size");
        }

        let staging_path = upload_staging_path(paths, upload_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&staging_path)
            .with_context(|| format!("failed to open staged upload {}", upload_id))?;
        use std::io::Write as _;
        file.write_all(bytes)?;
        upload.received_bytes = next_size;
        upload.expires_at = now_ts() + UPLOAD_TTL_SECS;

        Ok(AttachmentUploadChunkOutput {
            upload_id: upload_id.to_string(),
            received_bytes: upload.received_bytes,
            size_bytes: upload.size_bytes,
            complete: upload.received_bytes == upload.size_bytes,
        })
    })
}

pub fn finish_attachment_upload(
    data_dir: &Path,
    token: &str,
    upload_id: &str,
) -> anyhow::Result<AppendedConversationObject> {
    let (output, staged_path) = with_locked_state(data_dir, |paths, state| {
        let (sender, sender_member_did, sender_actor_id, current_session) = {
            let session = validate_session(state, token)?;
            session.last_seen_at = now_ts();
            (
                session.display_name.clone(),
                session.member_did.clone(),
                session.actor_id.clone(),
                current_session_identity(session),
            )
        };
        let upload_index = state
            .uploads
            .iter()
            .position(|upload| upload.upload_id == upload_id && upload.token == token)
            .ok_or_else(|| anyhow::anyhow!("upload not found"))?;
        let upload = state.uploads[upload_index].clone();
        if upload.received_bytes != upload.size_bytes {
            anyhow::bail!(
                "upload incomplete: received {} of {} bytes",
                upload.received_bytes,
                upload.size_bytes
            );
        }

        let staged_path = upload_staging_path(paths, &upload.upload_id);
        let bytes = fs::read(&staged_path)
            .with_context(|| format!("failed to read staged upload {}", upload.upload_id))?;
        let object = append_attachment_record(
            paths,
            state,
            AttachmentRecordInput {
                sender,
                sender_member_did: sender_member_did.clone(),
                sender_actor_id,
                created_at: now_ts(),
                file_name: &upload.file_name,
                mime_type: &upload.mime_type,
                bytes: &bytes,
            },
        )?;
        state.uploads.remove(upload_index);
        let transport_envelope = transport_envelope_from_record(paths, state, &object);
        Ok((
            AppendedConversationObject {
                object: object_view_from_record(object, Some(&current_session)),
                sender_member_did,
                transport_envelope,
            },
            staged_path,
        ))
    })?;
    let _ = fs::remove_file(staged_path);
    Ok(output)
}

pub fn read_attachment(
    data_dir: &Path,
    token: &str,
    attachment_id: &str,
) -> anyhow::Result<(AttachmentView, Vec<u8>)> {
    let attachment = with_locked_state(data_dir, |_, state| {
        let session = validate_session(state, token)?;
        session.last_seen_at = now_ts();
        state
            .objects
            .iter()
            .filter_map(|object| object.attachment.as_ref())
            .find(|attachment| attachment.attachment_id == attachment_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("attachment not found"))
    })?;
    let bytes = fs::read(storage_paths(data_dir)?.attachments_dir.join(attachment_id))
        .with_context(|| format!("failed to read attachment {}", attachment_id))?;
    Ok((attachment, bytes))
}

fn validate_session<'a>(
    state: &'a mut RoomState,
    token: &str,
) -> anyhow::Result<&'a mut SessionRecord> {
    let session = state
        .sessions
        .iter_mut()
        .find(|item| item.token == token)
        .ok_or_else(|| anyhow::anyhow!("invalid or expired session"))?;
    ensure_session_actor_id(session);
    if !session
        .capabilities
        .iter()
        .any(|capability| capability == ROOM_ACCESS_CAPABILITY)
    {
        anyhow::bail!("session is not approved for room access");
    }
    Ok(session)
}

fn participant_views_from_state(
    state: &RoomState,
    current_session: Option<&CurrentSessionIdentity>,
) -> Vec<ParticipantView> {
    #[derive(Debug, Clone)]
    struct ParticipantAggregate {
        display_name: String,
        device_label: String,
        last_seen_at: u64,
        member_did: Option<String>,
        role: Option<RoomRole>,
        local_session_count: usize,
        active_in_room: bool,
        is_current_session: bool,
    }

    let mut members = BTreeMap::<String, ParticipantAggregate>::new();

    for object in &state.objects {
        let Some(member_did) = object
            .sender_member_did
            .as_deref()
            .map(str::trim)
            .filter(|did| !did.is_empty())
        else {
            continue;
        };

        let participant_key = participant_member_key(member_did);
        let participant = members
            .entry(participant_key)
            .or_insert_with(|| ParticipantAggregate {
                display_name: default_member_display_name(
                    member_did,
                    active_member_record(state, member_did).map(|member| &member.role),
                ),
                device_label: default_member_device_label(
                    active_member_record(state, member_did).map(|member| &member.role),
                ),
                last_seen_at: 0,
                member_did: Some(member_did.to_string()),
                role: active_member_record(state, member_did).map(|member| member.role.clone()),
                local_session_count: 0,
                active_in_room: false,
                is_current_session: current_session
                    .map(|session| {
                        if !object.sender_actor_id.trim().is_empty() {
                            session.actor_id == object.sender_actor_id
                        } else {
                            session.member_did.as_deref() == Some(member_did)
                        }
                    })
                    .unwrap_or(false),
            });

        if !object.sender.trim().is_empty() {
            participant.display_name = object.sender.clone();
        }
        match object.kind {
            ConversationObjectKind::System => {
                if matches!(
                    object.body.as_deref(),
                    Some("left the room") | Some("was removed from the room in Home")
                ) {
                    participant.active_in_room = false;
                } else if matches!(object.body.as_deref(), Some("joined the room")) {
                    participant.active_in_room = true;
                }
            }
            ConversationObjectKind::Text
            | ConversationObjectKind::Emoji
            | ConversationObjectKind::Link => {
                participant.active_in_room = true;
            }
            ConversationObjectKind::Attachment => {}
        }
        participant.last_seen_at = participant.last_seen_at.max(object.created_at);
    }

    let mut guests = Vec::new();
    for session in &state.sessions {
        let Some(member_did) = session
            .member_did
            .as_deref()
            .map(str::trim)
            .filter(|did| !did.is_empty())
        else {
            guests.push(ParticipantView {
                display_name: session.display_name.clone(),
                device_label: session.device_label.clone(),
                last_seen_at: session.last_seen_at,
                member_did: None,
                role: None,
                local_session_count: 1,
                is_current_session: current_session
                    .map(|current| current.actor_id == session.actor_id)
                    .unwrap_or(false),
            });
            continue;
        };

        let participant_key = participant_member_key(member_did);
        let participant = members
            .entry(participant_key)
            .or_insert_with(|| ParticipantAggregate {
                display_name: session.display_name.clone(),
                device_label: session.device_label.clone(),
                last_seen_at: session.last_seen_at,
                member_did: Some(member_did.to_string()),
                role: active_member_record(state, member_did).map(|member| member.role.clone()),
                local_session_count: 0,
                active_in_room: false,
                is_current_session: current_session
                    .map(|current| current.actor_id == session.actor_id)
                    .unwrap_or(false),
            });

        participant.local_session_count += 1;
        participant.active_in_room = true;
        if current_session
            .map(|current| current.actor_id == session.actor_id)
            .unwrap_or(false)
        {
            participant.is_current_session = true;
        }
        if session.last_seen_at >= participant.last_seen_at || participant.local_session_count == 1
        {
            participant.display_name = session.display_name.clone();
            participant.device_label = session.device_label.clone();
        }
        participant.last_seen_at = participant.last_seen_at.max(session.last_seen_at);
    }

    let mut participants = members
        .into_values()
        .filter(|participant| participant.active_in_room)
        .map(|participant| ParticipantView {
            display_name: participant.display_name,
            device_label: participant.device_label,
            last_seen_at: participant.last_seen_at,
            member_did: participant.member_did,
            role: participant.role,
            local_session_count: participant.local_session_count,
            is_current_session: participant.is_current_session,
        })
        .collect::<Vec<_>>();
    participants.extend(guests);
    participants.sort_by(|left, right| {
        participant_role_rank(left.role.as_ref())
            .cmp(&participant_role_rank(right.role.as_ref()))
            .then_with(|| right.local_session_count.cmp(&left.local_session_count))
            .then_with(|| {
                left.display_name
                    .to_lowercase()
                    .cmp(&right.display_name.to_lowercase())
            })
            .then_with(|| left.device_label.cmp(&right.device_label))
            .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
    });
    participants
}

fn participant_role_rank(role: Option<&RoomRole>) -> u8 {
    match role {
        Some(RoomRole::Owner) => 0,
        Some(RoomRole::Admin) => 1,
        Some(RoomRole::Member) => 2,
        None => 3,
    }
}

fn participant_member_key(member_did: &str) -> String {
    format!("member:{member_did}")
}

fn default_member_display_name(member_did: &str, role: Option<&RoomRole>) -> String {
    match role {
        Some(RoomRole::Owner) => "ElastOS user".to_string(),
        Some(RoomRole::Admin) | Some(RoomRole::Member) | None => {
            format!("ElastOS user {}", short_did_suffix(member_did))
        }
    }
}

fn default_member_device_label(role: Option<&RoomRole>) -> String {
    match role {
        Some(RoomRole::Owner) => "primary ElastOS device".to_string(),
        Some(RoomRole::Admin) | Some(RoomRole::Member) | None => "ElastOS device".to_string(),
    }
}

fn short_did_suffix(member_did: &str) -> String {
    let tail = member_did.rsplit(':').next().unwrap_or(member_did);
    tail.chars()
        .rev()
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn pending_request_views_from_state(state: &RoomState) -> Vec<PendingRequestView> {
    let mut items = state
        .pending_requests
        .iter()
        .filter(|item| item.status == BrowserAccessStatus::Pending)
        .map(|item| PendingRequestView {
            request_id: item.request_id.clone(),
            display_name: item.display_name.clone(),
            device_label: item.device_label.clone(),
            requested_at: item.requested_at,
            capabilities: item.capabilities.clone(),
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.requested_at
            .cmp(&right.requested_at)
            .then_with(|| left.display_name.cmp(&right.display_name))
            .then_with(|| left.device_label.cmp(&right.device_label))
    });
    items
}

fn active_session_views_from_state(state: &RoomState) -> Vec<ActiveSessionView> {
    let mut items = state
        .sessions
        .iter()
        .map(|session| ActiveSessionView {
            session_id: session_public_id(&session.token),
            token: session.token.clone(),
            display_name: session.display_name.clone(),
            device_label: session.device_label.clone(),
            approved_at: session.approved_at,
            expires_at: session.expires_at,
            last_seen_at: session.last_seen_at,
            capabilities: session.capabilities.clone(),
            member_did: session.member_did.clone(),
        })
        .collect::<Vec<_>>();
    items.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.device_label.cmp(&right.device_label))
            .then_with(|| left.approved_at.cmp(&right.approved_at))
    });
    items
}

fn session_public_id(token: &str) -> String {
    let digest = sha2::Sha256::digest(format!("elastos.room.session.v1:{token}").as_bytes());
    hex::encode(digest)[..32].to_string()
}

fn normalize_session_id(input: &str) -> anyhow::Result<String> {
    let value = input.trim().to_ascii_lowercase();
    if value.len() != 32 || !value.chars().all(|ch| ch.is_ascii_hexdigit()) {
        anyhow::bail!("invalid room session id");
    }
    Ok(value)
}

fn local_runtime_access_from_state(
    state: &RoomState,
    runtime_did: Option<&str>,
) -> LocalRuntimeAccess {
    if !state.control.allow_guest_invites {
        return LocalRuntimeAccess {
            runtime_did: runtime_did.map(|did| did.to_string()),
            member_role: runtime_did
                .and_then(|did| active_member_record(state, did))
                .map(|member| member.role.clone()),
            browser_access_allowed: false,
            block_reason: Some("Hosted guest access is disabled for this room.".to_string()),
        };
    }

    if !room_requires_active_membership(state) {
        return LocalRuntimeAccess {
            runtime_did: runtime_did.map(|did| did.to_string()),
            member_role: None,
            browser_access_allowed: true,
            block_reason: None,
        };
    }

    let Some(runtime_did) = runtime_did else {
        return LocalRuntimeAccess {
            runtime_did: None,
            member_role: None,
            browser_access_allowed: false,
            block_reason: Some(
                "This runtime has no active room member DID available for pairing.".to_string(),
            ),
        };
    };

    let role = active_member_record(state, runtime_did).map(|member| member.role.clone());
    if let Some(role) = role {
        if role == RoomRole::Member && !state.control.allow_members_to_host_guests {
            return LocalRuntimeAccess {
                runtime_did: Some(runtime_did.to_string()),
                member_role: Some(role),
                browser_access_allowed: false,
                block_reason: Some(
                    "This conversation only lets managers approve web guests.".to_string(),
                ),
            };
        }
        LocalRuntimeAccess {
            runtime_did: Some(runtime_did.to_string()),
            member_role: Some(role),
            browser_access_allowed: true,
            block_reason: None,
        }
    } else {
        LocalRuntimeAccess {
            runtime_did: Some(runtime_did.to_string()),
            member_role: None,
            browser_access_allowed: false,
            block_reason: Some("This device is not part of this conversation yet.".to_string()),
        }
    }
}

fn room_requires_active_membership(state: &RoomState) -> bool {
    state.control.owner_did.is_some() || state.members.iter().any(|member| member.active)
}

fn ensure_browser_access_allowed_for_member(
    state: &RoomState,
    member_did: Option<&str>,
) -> anyhow::Result<()> {
    let access = local_runtime_access_from_state(state, member_did);
    if access.browser_access_allowed {
        Ok(())
    } else {
        anyhow::bail!(
            "{}",
            access
                .block_reason
                .unwrap_or_else(|| "web guest access is not allowed from this device".to_string())
        )
    }
}

fn room_control_summary_from_state(state: &RoomState) -> RoomControlSummary {
    let mut members = state
        .members
        .iter()
        .filter(|member| member.active)
        .map(room_member_view_from_record)
        .collect::<Vec<_>>();
    members.sort_by(|left, right| {
        role_sort_key(&left.role)
            .cmp(&role_sort_key(&right.role))
            .then_with(|| left.member_did.cmp(&right.member_did))
    });
    let mut pending_invites = state
        .invites
        .iter()
        .filter(|invite| invite.status == InviteStatus::Pending)
        .map(room_invite_view_from_record)
        .collect::<Vec<_>>();
    pending_invites.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.invited_did.cmp(&right.invited_did))
    });
    let mut key_epochs = state
        .key_epochs
        .iter()
        .cloned()
        .map(room_key_epoch_view_from_record)
        .collect::<Vec<_>>();
    key_epochs.sort_by(|left, right| left.epoch.cmp(&right.epoch));
    RoomControlSummary {
        title: state.control.title.clone(),
        owner_did: state.control.owner_did.clone(),
        current_key_epoch: state.control.current_key_epoch,
        admin_count: state
            .members
            .iter()
            .filter(|member| member.active && member.role == RoomRole::Admin)
            .count(),
        member_count: state.members.iter().filter(|member| member.active).count(),
        active_member_count: state.members.iter().filter(|member| member.active).count(),
        access_policy: access_policy_view_from_state(state),
        members,
        pending_invites,
        key_epochs,
    }
}

fn access_policy_view_from_state(state: &RoomState) -> RoomAccessPolicyView {
    RoomAccessPolicyView {
        allow_guest_invites: state.control.allow_guest_invites,
        allow_member_invites: state.control.allow_member_invites,
        allow_members_to_host_guests: state.control.allow_members_to_host_guests,
    }
}

fn room_member_view_from_record(record: &RoomMemberRecord) -> RoomMemberView {
    RoomMemberView {
        member_did: record.member_did.clone(),
        role: record.role.clone(),
        added_at: record.added_at,
        added_by: record.added_by.clone(),
        profile_card: record.profile_card.clone(),
    }
}

fn room_invite_view_from_record(record: &RoomInviteRecord) -> RoomInviteView {
    RoomInviteView {
        invite_id: record.invite_id.clone(),
        invited_did: record.invited_did.clone(),
        role: record.role.clone(),
        invited_by: record.invited_by.clone(),
        created_at: record.created_at,
        expires_at: record.expires_at,
    }
}

fn room_key_epoch_view_from_record(record: RoomKeyEpochRecord) -> RoomKeyEpochView {
    RoomKeyEpochView {
        epoch: record.epoch,
        created_at: record.created_at,
        created_by: record.created_by,
        reason: record.reason,
    }
}

fn active_member_record<'a>(state: &'a RoomState, did: &str) -> Option<&'a RoomMemberRecord> {
    state
        .members
        .iter()
        .find(|member| member.active && member.member_did == did)
}

fn active_member_record_mut<'a>(
    state: &'a mut RoomState,
    did: &str,
) -> Option<&'a mut RoomMemberRecord> {
    state
        .members
        .iter_mut()
        .find(|member| member.active && member.member_did == did)
}

fn require_active_member_role(state: &RoomState, did: &str) -> anyhow::Result<RoomRole> {
    active_member_record(state, did)
        .map(|member| member.role.clone())
        .ok_or_else(|| anyhow::anyhow!("member DID is not active in this room"))
}

fn require_room_owner_seeded(state: &RoomState) -> anyhow::Result<()> {
    if state.control.owner_did.is_none() {
        anyhow::bail!("room owner is not seeded yet");
    }
    Ok(())
}

fn ensure_active_member<'a>(
    state: &'a mut RoomState,
    member_did: &str,
    role: RoomRole,
    added_by: &str,
    now: u64,
) -> &'a RoomMemberRecord {
    ensure_active_member_with_profile(state, member_did, role, added_by, now, None)
}

fn ensure_active_member_with_profile<'a>(
    state: &'a mut RoomState,
    member_did: &str,
    role: RoomRole,
    added_by: &str,
    now: u64,
    profile_card: Option<RoomProfileCardView>,
) -> &'a RoomMemberRecord {
    if let Some(index) = state
        .members
        .iter()
        .position(|member| member.member_did == member_did)
    {
        let member = &mut state.members[index];
        member.role = role;
        member.active = true;
        member.removed_at = None;
        member.removed_by = None;
        if profile_card.is_some() {
            member.profile_card = profile_card;
        }
        return member;
    }
    state.members.push(RoomMemberRecord {
        member_did: member_did.to_string(),
        role,
        added_at: now,
        added_by: added_by.to_string(),
        active: true,
        profile_card,
        removed_at: None,
        removed_by: None,
    });
    state.members.last().expect("member just pushed")
}

fn rotate_key_epoch_record(
    state: &mut RoomState,
    actor_did: &str,
    reason: String,
    now: u64,
) -> RoomKeyEpochRecord {
    let next_epoch = state
        .key_epochs
        .last()
        .map(|item| item.epoch + 1)
        .unwrap_or(1);
    let record = RoomKeyEpochRecord {
        epoch: next_epoch,
        created_at: now,
        created_by: actor_did.to_string(),
        reason,
    };
    state.key_epochs.push(record.clone());
    state.control.current_key_epoch = next_epoch;
    state.control.updated_at = now;
    record
}

fn can_invite_role(actor_role: &RoomRole, invited_role: &RoomRole) -> bool {
    match actor_role {
        RoomRole::Owner => matches!(invited_role, RoomRole::Admin | RoomRole::Member),
        RoomRole::Admin => matches!(invited_role, RoomRole::Member),
        RoomRole::Member => matches!(invited_role, RoomRole::Member),
    }
}

fn can_remove_role(actor_role: &RoomRole, target_role: &RoomRole) -> bool {
    match actor_role {
        RoomRole::Owner => matches!(target_role, RoomRole::Admin | RoomRole::Member),
        RoomRole::Admin => matches!(target_role, RoomRole::Member),
        RoomRole::Member => false,
    }
}

fn actor_role_label(role: &RoomRole) -> &'static str {
    match role {
        RoomRole::Owner => "owner",
        RoomRole::Admin => "admin",
        RoomRole::Member => "member",
    }
}

fn role_label(role: &RoomRole) -> &'static str {
    actor_role_label(role)
}

fn role_sort_key(role: &RoomRole) -> u8 {
    match role {
        RoomRole::Owner => 0,
        RoomRole::Admin => 1,
        RoomRole::Member => 2,
    }
}

fn room_access_policy_enabled_default() -> bool {
    true
}

fn summary_browser_access_allowed_default() -> bool {
    true
}

fn find_upload_mut<'a>(
    state: &'a mut RoomState,
    token: &str,
    upload_id: &str,
) -> anyhow::Result<&'a mut UploadRecord> {
    state
        .uploads
        .iter_mut()
        .find(|upload| upload.upload_id == upload_id && upload.token == token)
        .ok_or_else(|| anyhow::anyhow!("upload not found"))
}

fn next_pending_request(state: &RoomState) -> Option<&BrowserAccessRequestRecord> {
    state
        .pending_requests
        .iter()
        .find(|item| item.status == BrowserAccessStatus::Pending)
}

fn invalidate_approved_requests(state: &mut RoomState, tokens: &[String]) {
    if tokens.is_empty() {
        return;
    }
    for request in &mut state.pending_requests {
        let matches_token = request
            .session_token
            .as_ref()
            .is_some_and(|token| tokens.iter().any(|candidate| candidate == token));
        if request.status == BrowserAccessStatus::Approved && matches_token {
            request.status = BrowserAccessStatus::Expired;
            request.session_token = None;
            request.session_expires_at = None;
            request.denial_reason = None;
        }
    }
}

fn remove_uploads_for_tokens(paths: &RoomPaths, state: &mut RoomState, tokens: &[String]) {
    if tokens.is_empty() {
        return;
    }
    let mut removed_ids = Vec::new();
    state.uploads.retain(|upload| {
        let keep = !tokens.iter().any(|candidate| candidate == &upload.token);
        if !keep {
            removed_ids.push(upload.upload_id.clone());
        }
        keep
    });
    for upload_id in removed_ids {
        let _ = fs::remove_file(upload_staging_path(paths, &upload_id));
    }
}

fn count_dir_entries(path: &Path) -> anyhow::Result<usize> {
    if !path.exists() {
        return Ok(0);
    }
    Ok(fs::read_dir(path)?.filter_map(Result::ok).count())
}

fn remove_dir_all_if_exists(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("failed to remove {}", path.display()))?;
    }
    Ok(())
}

struct AttachmentRecordInput<'a> {
    sender: String,
    sender_member_did: Option<String>,
    sender_actor_id: String,
    created_at: u64,
    file_name: &'a str,
    mime_type: &'a str,
    bytes: &'a [u8],
}

fn append_attachment_record(
    paths: &RoomPaths,
    state: &mut RoomState,
    input: AttachmentRecordInput<'_>,
) -> anyhow::Result<ConversationObjectRecord> {
    let attachment_id = random_hex(16);
    fs::create_dir_all(&paths.attachments_dir)?;
    write_bytes_atomic(&paths.attachments_dir.join(&attachment_id), input.bytes)?;
    let attachment = AttachmentView {
        attachment_id,
        file_name: input.file_name.to_string(),
        mime_type: input.mime_type.to_string(),
        size_bytes: input.bytes.len() as u64,
        is_image: input.mime_type.starts_with("image/"),
        is_audio: input.mime_type.starts_with("audio/"),
        is_video: input.mime_type.starts_with("video/"),
    };
    let object = push_object(
        state,
        ConversationObjectRecord {
            seq: 0,
            event_id: new_object_event_id(),
            sender: input.sender,
            sender_member_did: input.sender_member_did,
            sender_actor_id: input.sender_actor_id,
            kind: ConversationObjectKind::Attachment,
            body: None,
            emoji: None,
            link: None,
            attachment: Some(attachment),
            created_at: input.created_at,
        },
    );
    Ok(object)
}

fn push_system_object(
    state: &mut RoomState,
    sender: String,
    body: String,
    created_at: u64,
) -> ConversationObjectRecord {
    push_object(
        state,
        ConversationObjectRecord {
            seq: 0,
            event_id: new_object_event_id(),
            sender,
            sender_member_did: None,
            sender_actor_id: String::new(),
            kind: ConversationObjectKind::System,
            body: Some(body),
            emoji: None,
            link: None,
            attachment: None,
            created_at,
        },
    )
}

fn push_member_system_object(
    state: &mut RoomState,
    member_did: &str,
    sender: String,
    body: String,
    created_at: u64,
) -> ConversationObjectRecord {
    push_member_system_object_for_actor(state, member_did, None, sender, body, created_at)
}

fn push_member_system_object_for_actor(
    state: &mut RoomState,
    member_did: &str,
    actor_id: Option<&str>,
    sender: String,
    body: String,
    created_at: u64,
) -> ConversationObjectRecord {
    push_object(
        state,
        ConversationObjectRecord {
            seq: 0,
            event_id: new_object_event_id(),
            sender,
            sender_member_did: Some(member_did.to_string()),
            sender_actor_id: actor_id.unwrap_or_default().to_string(),
            kind: ConversationObjectKind::System,
            body: Some(body),
            emoji: None,
            link: None,
            attachment: None,
            created_at,
        },
    )
}

fn push_object(
    state: &mut RoomState,
    mut object: ConversationObjectRecord,
) -> ConversationObjectRecord {
    if object.event_id.trim().is_empty() {
        object.event_id = new_object_event_id();
    }
    object.seq = state.next_seq;
    state.next_seq += 1;
    state.objects.push(object.clone());
    if state.objects.len() > MAX_OBJECTS {
        let drop_count = state.objects.len() - MAX_OBJECTS;
        state.objects.drain(0..drop_count);
    }
    object
}

fn object_view_from_record(
    object: ConversationObjectRecord,
    current_session: Option<&CurrentSessionIdentity>,
) -> ConversationObjectView {
    let from_current_session = current_session
        .map(|session| {
            if !object.sender_actor_id.trim().is_empty() {
                object.sender_actor_id == session.actor_id
            } else {
                object
                    .sender_member_did
                    .as_deref()
                    .zip(session.member_did.as_deref())
                    .map(|(left, right)| left == right)
                    .unwrap_or(false)
            }
        })
        .unwrap_or(false);
    ConversationObjectView {
        seq: object.seq,
        sender: object.sender,
        sender_member_did: object.sender_member_did,
        from_current_session,
        kind: object.kind,
        body: object.body,
        emoji: object.emoji,
        link: object.link,
        attachment: object.attachment,
        created_at: object.created_at,
    }
}

fn transport_envelope_from_record(
    paths: &RoomPaths,
    state: &RoomState,
    object: &ConversationObjectRecord,
) -> Option<RoomObjectEnvelope> {
    if !is_transportable_room_object_kind(&object.kind) {
        return None;
    }
    let sender_member_did = object.sender_member_did.as_ref()?.trim();
    if sender_member_did.is_empty() {
        return None;
    }
    let (attachment, attachment_bytes_b64) = match object.kind {
        ConversationObjectKind::Attachment => {
            let attachment = object.attachment.clone()?;
            let bytes = fs::read(paths.attachments_dir.join(&attachment.attachment_id)).ok()?;
            (Some(attachment), Some(BASE64_STANDARD.encode(bytes)))
        }
        _ => (None, None),
    };
    Some(RoomObjectEnvelope {
        schema: ROOM_OBJECT_ENVELOPE_SCHEMA.to_string(),
        room_slug: state.room_slug.clone(),
        event_id: object.event_id.clone(),
        sender: object.sender.clone(),
        sender_member_did: sender_member_did.to_string(),
        kind: object.kind.clone(),
        body: object.body.clone(),
        emoji: object.emoji.clone(),
        link: object.link.clone(),
        attachment,
        attachment_bytes_b64,
        created_at: object.created_at,
    })
}

fn with_locked_state<T>(
    data_dir: &Path,
    f: impl FnOnce(&RoomPaths, &mut RoomState) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let paths = storage_paths(data_dir)?;
    fs::create_dir_all(&paths.root_dir)?;

    let mut lockfile = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&paths.lock_path)
        .with_context(|| format!("failed to open lockfile {}", paths.lock_path.display()))?;

    flock_exclusive(&lockfile)?;

    let mut state = load_state(&paths)?;
    prune_state(&paths, &mut state);
    let result = f(&paths, &mut state)?;
    save_state(&paths, &state)?;
    unlock_file(&lockfile)?;
    let _ = lockfile.seek(SeekFrom::Start(0));
    Ok(result)
}

fn load_state(paths: &RoomPaths) -> anyhow::Result<RoomState> {
    if split_store_exists(paths) {
        let mut state = load_split_state(paths)?;
        normalize_state_defaults(&mut state);
        return Ok(state);
    }
    let mut state = RoomState::default();
    normalize_state_defaults(&mut state);
    Ok(state)
}

fn load_split_state(paths: &RoomPaths) -> anyhow::Result<RoomState> {
    let meta: RoomMeta = read_json_or_default(&paths.room_meta_path)?;
    let control: RoomControlRecord = read_json_or_default(&paths.control_path)?;
    let members: Vec<RoomMemberRecord> = read_json_or_default(&paths.members_path)?;
    let invites: Vec<RoomInviteRecord> = read_json_or_default(&paths.invites_path)?;
    let key_epochs: Vec<RoomKeyEpochRecord> = read_json_or_default(&paths.key_epochs_path)?;
    let pending_requests: Vec<BrowserAccessRequestRecord> =
        read_json_or_default(&paths.pair_requests_path)?;
    let sessions: Vec<SessionRecord> = read_json_or_default(&paths.sessions_path)?;
    let objects: Vec<ConversationObjectRecord> = read_json_or_default(&paths.objects_path)?;
    let uploads: Vec<UploadRecord> = read_json_or_default(&paths.uploads_path)?;

    Ok(RoomState {
        schema: if meta.schema.trim().is_empty() {
            STATE_SCHEMA.to_string()
        } else {
            meta.schema
        },
        room_slug: if meta.room_slug.trim().is_empty() {
            ROOM_SLUG.to_string()
        } else {
            meta.room_slug
        },
        next_seq: if meta.next_seq == 0 { 1 } else { meta.next_seq },
        control,
        members,
        invites,
        key_epochs,
        pending_requests,
        sessions,
        objects,
        uploads,
    })
}

fn save_state(paths: &RoomPaths, state: &RoomState) -> anyhow::Result<()> {
    fs::create_dir_all(&paths.root_dir)?;
    fs::create_dir_all(&paths.room_dir)?;
    fs::create_dir_all(&paths.local_dir)?;
    write_json_atomic(
        &paths.room_meta_path,
        &RoomMeta {
            schema: state.schema.clone(),
            room_slug: state.room_slug.clone(),
            next_seq: state.next_seq,
        },
    )?;
    write_json_atomic(&paths.control_path, &state.control)?;
    write_json_atomic(&paths.members_path, &state.members)?;
    write_json_atomic(&paths.invites_path, &state.invites)?;
    write_json_atomic(&paths.key_epochs_path, &state.key_epochs)?;
    write_json_atomic(&paths.pair_requests_path, &state.pending_requests)?;
    write_json_atomic(&paths.sessions_path, &state.sessions)?;
    write_json_atomic(&paths.objects_path, &state.objects)?;
    write_json_atomic(&paths.uploads_path, &state.uploads)?;
    Ok(())
}

fn prune_state(paths: &RoomPaths, state: &mut RoomState) {
    let now = now_ts();
    for request in &mut state.pending_requests {
        if request.status == BrowserAccessStatus::Pending && request.expires_at <= now {
            request.status = BrowserAccessStatus::Expired;
            request.session_token = None;
            request.session_expires_at = None;
        }
        if request.status == BrowserAccessStatus::Approved
            && request
                .session_expires_at
                .is_some_and(|expires_at| expires_at <= now)
        {
            request.status = BrowserAccessStatus::Expired;
            request.session_token = None;
        }
    }
    for invite in &mut state.invites {
        if invite.status == InviteStatus::Pending && invite.expires_at <= now {
            invite.status = InviteStatus::Expired;
            invite.acted_at = Some(now);
        }
    }
    state.sessions.retain(|session| session.expires_at > now);
    let active_tokens = state
        .sessions
        .iter()
        .map(|session| session.token.clone())
        .collect::<Vec<_>>();
    let mut removed_uploads = Vec::new();
    state.uploads.retain(|upload| {
        let keep =
            upload.expires_at > now && active_tokens.iter().any(|token| token == &upload.token);
        if !keep {
            removed_uploads.push(upload.upload_id.clone());
        }
        keep
    });
    for upload_id in removed_uploads {
        let _ = fs::remove_file(upload_staging_path(paths, &upload_id));
    }
}

fn storage_paths(data_dir: &Path) -> anyhow::Result<RoomPaths> {
    let root_dir = rooted_localhost_fs_path(data_dir, ROOM_ROOT_URI)
        .ok_or_else(|| anyhow::anyhow!("invalid room root URI {}", ROOM_ROOT_URI))?;
    let room_dir = root_dir.join(ROOM_SHARED_DIR);
    let local_dir = root_dir.join(ROOM_LOCAL_DIR);
    Ok(RoomPaths {
        room_dir: room_dir.clone(),
        local_dir: local_dir.clone(),
        lock_path: root_dir.join(ROOM_LOCK_FILE),
        room_meta_path: room_dir.join(ROOM_META_FILE),
        control_path: room_dir.join(ROOM_CONTROL_FILE),
        members_path: room_dir.join(ROOM_MEMBERS_FILE),
        invites_path: room_dir.join(ROOM_INVITES_FILE),
        key_epochs_path: room_dir.join(ROOM_KEY_EPOCHS_FILE),
        pair_requests_path: local_dir.join(BROWSER_ACCESS_REQUESTS_FILE),
        sessions_path: local_dir.join(ROOM_SESSIONS_FILE),
        objects_path: room_dir.join(ROOM_OBJECTS_FILE),
        uploads_path: local_dir.join(ROOM_UPLOADS_FILE),
        attachments_dir: room_dir.join(ROOM_ATTACHMENTS_DIR),
        uploads_dir: local_dir.join(ROOM_UPLOADS_DIR),
        root_dir,
    })
}

fn split_store_exists(paths: &RoomPaths) -> bool {
    paths.room_meta_path.exists()
        || paths.control_path.exists()
        || paths.members_path.exists()
        || paths.invites_path.exists()
        || paths.key_epochs_path.exists()
        || paths.pair_requests_path.exists()
        || paths.sessions_path.exists()
        || paths.objects_path.exists()
        || paths.uploads_path.exists()
}

fn normalize_state_defaults(state: &mut RoomState) {
    if state.control.created_at == 0 {
        state.control.created_at = now_ts();
    }
    if state.control.updated_at == 0 {
        state.control.updated_at = state.control.created_at;
    }
    if state.control.schema.trim().is_empty() {
        state.control.schema = STATE_SCHEMA.to_string();
    }
    if state.control.room_slug.trim().is_empty() {
        state.control.room_slug = ROOM_SLUG.to_string();
    }
    if state.control.title.trim().is_empty() {
        state.control.title = "Room".to_string();
    }
    if state.control.current_key_epoch == 0 {
        state.control.current_key_epoch = 1;
    }
    if state.key_epochs.is_empty() {
        state.key_epochs.push(RoomKeyEpochRecord {
            epoch: state.control.current_key_epoch,
            created_at: state.control.created_at,
            created_by: state
                .control
                .owner_did
                .clone()
                .unwrap_or_else(|| "unseeded-room".to_string()),
            reason: "initial room epoch".to_string(),
        });
    }
    if let Some(owner_did) = state.control.owner_did.clone() {
        let created_at = state.control.created_at;
        ensure_active_member(state, &owner_did, RoomRole::Owner, &owner_did, created_at);
    }
    for object in &mut state.objects {
        if object.event_id.trim().is_empty() {
            object.event_id = format!("event-{}", object.seq.max(1));
        }
    }
}

fn upload_staging_path(paths: &RoomPaths, upload_id: &str) -> PathBuf {
    paths.uploads_dir.join(upload_id)
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
            .unwrap_or("state")
    ));
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn write_bytes_atomic(path: &Path, value: &[u8]) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path missing parent"))?;
    fs::create_dir_all(parent)?;
    let tmp = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("blob")
    ));
    fs::write(&tmp, value)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn now_ts() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut raw);
    hex::encode(raw)
}

fn normalize_display_name(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("display name must not be empty");
    }
    if trimmed.chars().count() > 48 {
        anyhow::bail!("display name must be 48 characters or fewer");
    }
    Ok(trimmed.to_string())
}

fn normalize_member_did(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("member DID must not be empty");
    }
    if !trimmed.starts_with("did:") {
        anyhow::bail!("member DID must start with did:");
    }
    if trimmed.chars().count() > 240 {
        anyhow::bail!("member DID must be 240 characters or fewer");
    }
    Ok(trimmed.to_string())
}

fn normalize_room_title(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("room title must not be empty");
    }
    if trimmed.chars().count() > 80 {
        anyhow::bail!("room title must be 80 characters or fewer");
    }
    Ok(trimmed.to_string())
}

fn normalize_key_rotation_reason(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("key rotation reason must not be empty");
    }
    if trimmed.chars().count() > 200 {
        anyhow::bail!("key rotation reason must be 200 characters or fewer");
    }
    Ok(trimmed.to_string())
}

fn normalize_device_label(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        "browser".to_string()
    } else {
        trimmed.chars().take(64).collect()
    }
}

fn normalize_browser_session_capabilities(input: &[String]) -> anyhow::Result<Vec<String>> {
    if input.is_empty() {
        anyhow::bail!("browser session capabilities must not be empty");
    }
    let mut capabilities = input
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if capabilities.is_empty() {
        anyhow::bail!("browser session capabilities must not be empty");
    }
    capabilities.sort();
    capabilities.dedup();
    for capability in &capabilities {
        if capability != ROOM_ACCESS_CAPABILITY {
            anyhow::bail!("unsupported browser session capability: {}", capability);
        }
    }
    Ok(capabilities)
}

fn normalize_denial_reason(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        "Denied in Home".to_string()
    } else {
        trimmed.chars().take(120).collect()
    }
}

fn normalize_object_body(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("message must not be empty");
    }
    if trimmed.chars().count() > MAX_OBJECT_BODY_LEN {
        anyhow::bail!("message exceeds {} characters", MAX_OBJECT_BODY_LEN);
    }
    Ok(trimmed.to_string())
}

fn normalize_file_name(input: &str) -> String {
    let mut cleaned: String = input
        .trim()
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or("attachment.bin")
        .chars()
        .filter(|ch| !matches!(ch, '\0'..='\u{1f}' | '\u{7f}'))
        .take(120)
        .collect();
    if cleaned.trim().is_empty() {
        cleaned = "attachment.bin".to_string();
    }
    cleaned
}

fn normalize_attachment_id(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.len() != 32 || !trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        anyhow::bail!("attachment id must be a 32-character hex string");
    }
    Ok(trimmed.to_ascii_lowercase())
}

fn normalize_mime_type(input: &str) -> String {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return "application/octet-stream".to_string();
    }
    let cleaned: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '+' | '-' | '.' | ';' | '='))
        .take(120)
        .collect();
    if cleaned.is_empty() {
        "application/octet-stream".to_string()
    } else {
        cleaned
    }
}

fn normalize_room_object_event_id(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("room object event ID must not be empty");
    }
    if trimmed.chars().count() > 128 {
        anyhow::bail!("room object event ID must be 128 characters or fewer");
    }
    Ok(trimmed.to_string())
}

fn new_object_event_id() -> String {
    random_hex(16)
}

#[derive(Debug)]
struct ConversationObjectDraft {
    kind: ConversationObjectKind,
    body: Option<String>,
    emoji: Option<String>,
    link: Option<LinkPreviewView>,
    attachment: Option<AttachmentView>,
    attachment_bytes: Option<Vec<u8>>,
}

fn classify_object_body(input: &str) -> anyhow::Result<ConversationObjectDraft> {
    let body = normalize_object_body(input)?;
    if let Some(link) = classify_link_object(&body) {
        return Ok(ConversationObjectDraft {
            kind: ConversationObjectKind::Link,
            body: Some(body),
            emoji: None,
            link: Some(link),
            attachment: None,
            attachment_bytes: None,
        });
    }
    if is_emoji_only_message(&body) {
        return Ok(ConversationObjectDraft {
            kind: ConversationObjectKind::Emoji,
            body: None,
            emoji: Some(body),
            link: None,
            attachment: None,
            attachment_bytes: None,
        });
    }
    Ok(ConversationObjectDraft {
        kind: ConversationObjectKind::Text,
        body: Some(body),
        emoji: None,
        link: None,
        attachment: None,
        attachment_bytes: None,
    })
}

fn is_transportable_room_object_kind(kind: &ConversationObjectKind) -> bool {
    matches!(
        kind,
        ConversationObjectKind::System
            | ConversationObjectKind::Text
            | ConversationObjectKind::Emoji
            | ConversationObjectKind::Link
            | ConversationObjectKind::Attachment
    )
}

fn validate_room_object_envelope(envelope: &RoomObjectEnvelope) -> anyhow::Result<()> {
    if envelope.schema != ROOM_OBJECT_ENVELOPE_SCHEMA {
        anyhow::bail!("unsupported room object schema: {}", envelope.schema);
    }
    if envelope.room_slug != ROOM_SLUG {
        anyhow::bail!(
            "room object envelope is for room '{}' not '{}'",
            envelope.room_slug,
            ROOM_SLUG
        );
    }
    if !is_transportable_room_object_kind(&envelope.kind) {
        anyhow::bail!("room object kind {:?} is not transportable", envelope.kind);
    }
    if envelope.created_at == 0 {
        anyhow::bail!("room object created_at must be set");
    }
    let _ = normalize_room_object_event_id(&envelope.event_id)?;
    let _ = normalize_display_name(&envelope.sender)?;
    let _ = normalize_member_did(&envelope.sender_member_did)?;
    let _ = normalize_transport_object_payload(envelope)?;
    Ok(())
}

fn normalize_transport_object_payload(
    envelope: &RoomObjectEnvelope,
) -> anyhow::Result<ConversationObjectDraft> {
    match &envelope.kind {
        ConversationObjectKind::System => {
            let body = normalize_transport_system_body(
                envelope
                    .body
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("room system object missing body"))?,
            )?;
            Ok(ConversationObjectDraft {
                kind: ConversationObjectKind::System,
                body: Some(body),
                emoji: None,
                link: None,
                attachment: None,
                attachment_bytes: None,
            })
        }
        ConversationObjectKind::Text => Ok(ConversationObjectDraft {
            kind: ConversationObjectKind::Text,
            body: Some(normalize_object_body(
                envelope
                    .body
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("room text object missing body"))?,
            )?),
            emoji: None,
            link: None,
            attachment: None,
            attachment_bytes: None,
        }),
        ConversationObjectKind::Emoji => {
            let emoji = normalize_object_body(
                envelope
                    .emoji
                    .as_deref()
                    .or(envelope.body.as_deref())
                    .ok_or_else(|| anyhow::anyhow!("room emoji object missing emoji payload"))?,
            )?;
            if !is_emoji_only_message(&emoji) {
                anyhow::bail!("room emoji object must contain emoji-only content");
            }
            Ok(ConversationObjectDraft {
                kind: ConversationObjectKind::Emoji,
                body: None,
                emoji: Some(emoji),
                link: None,
                attachment: None,
                attachment_bytes: None,
            })
        }
        ConversationObjectKind::Link => {
            let body = normalize_object_body(
                envelope
                    .body
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("room link object missing body"))?,
            )?;
            let link = classify_link_object(&body)
                .ok_or_else(|| anyhow::anyhow!("room link object body is not a valid URL"))?;
            Ok(ConversationObjectDraft {
                kind: ConversationObjectKind::Link,
                body: Some(body),
                emoji: None,
                link: Some(link),
                attachment: None,
                attachment_bytes: None,
            })
        }
        ConversationObjectKind::Attachment => {
            let attachment = envelope
                .attachment
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("room attachment object missing metadata"))?;
            let attachment_id = normalize_attachment_id(&attachment.attachment_id)?;
            let file_name = normalize_file_name(&attachment.file_name);
            let mime_type = normalize_mime_type(&attachment.mime_type);
            let bytes = BASE64_STANDARD
                .decode(
                    envelope
                        .attachment_bytes_b64
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("room attachment object missing bytes"))?,
                )
                .map_err(|err| {
                    anyhow::anyhow!("room attachment object has invalid bytes: {err}")
                })?;
            if bytes.is_empty() {
                anyhow::bail!("room attachment object missing bytes");
            }
            if bytes.len() > MAX_ATTACHMENT_BYTES {
                anyhow::bail!(
                    "room attachment object exceeds {} bytes",
                    MAX_ATTACHMENT_BYTES
                );
            }
            Ok(ConversationObjectDraft {
                kind: ConversationObjectKind::Attachment,
                body: None,
                emoji: None,
                link: None,
                attachment: Some(AttachmentView {
                    attachment_id,
                    file_name,
                    mime_type: mime_type.clone(),
                    size_bytes: bytes.len() as u64,
                    is_image: mime_type.starts_with("image/"),
                    is_audio: mime_type.starts_with("audio/"),
                    is_video: mime_type.starts_with("video/"),
                }),
                attachment_bytes: Some(bytes),
            })
        }
    }
}

fn normalize_transport_system_body(input: &str) -> anyhow::Result<String> {
    let body = normalize_object_body(input)?;
    if matches!(
        body.as_str(),
        "joined the room" | "left the room" | "was removed from the room in Home"
    ) {
        Ok(body)
    } else {
        anyhow::bail!("unsupported transport system body: {body}");
    }
}

fn active_local_session_count(state: &RoomState, member_did: &str) -> usize {
    state
        .sessions
        .iter()
        .filter(|session| session.member_did.as_deref() == Some(member_did))
        .count()
}

fn classify_link_object(input: &str) -> Option<LinkPreviewView> {
    if input.contains(char::is_whitespace) {
        return None;
    }
    let parsed = Url::parse(input).ok()?;
    if parsed.scheme() == "elastos" {
        let cid = parsed.host_str()?.to_string();
        let mut title = "Published document".to_string();
        if !cid.trim().is_empty() {
            title.push_str(" / ");
            title.push_str(&short_link_label(&cid));
        }
        return Some(LinkPreviewView {
            url: input.to_string(),
            host: "Documents".to_string(),
            title,
        });
    }
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let host = parsed.host_str()?.to_string();
    let mut title = host.clone();
    let path = parsed.path().trim_matches('/');
    if !path.is_empty() {
        title.push_str(" / ");
        title.push_str(path);
    }
    Some(LinkPreviewView {
        url: input.to_string(),
        host,
        title,
    })
}

fn normalize_join_invite_gateway(input: &str) -> anyhow::Result<String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("conversation join invite gateway must not be empty");
    }
    let parsed = Url::parse(trimmed).context("conversation join invite gateway is not a URL")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        anyhow::bail!("conversation join invite gateway must use http or https");
    }
    if parsed.host_str().is_none() {
        anyhow::bail!("conversation join invite gateway must include a host");
    }
    Ok(parsed.origin().ascii_serialization())
}

fn short_link_label(value: &str) -> String {
    if value.len() <= 18 {
        value.to_string()
    } else {
        format!("{}…{}", &value[..10], &value[value.len() - 6..])
    }
}

fn is_emoji_only_message(input: &str) -> bool {
    let mut saw_visible = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            continue;
        }
        if matches!(ch, '\u{200D}' | '\u{FE0F}') || is_emoji_modifier(ch) {
            continue;
        }
        if !is_emoji_scalar(ch) {
            return false;
        }
        saw_visible = true;
    }
    saw_visible
}

fn is_emoji_modifier(ch: char) -> bool {
    matches!(ch as u32, 0x1F3FB..=0x1F3FF)
}

fn is_emoji_scalar(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2600..=0x27BF | 0x1F300..=0x1FAFF | 0x1F1E6..=0x1F1FF
    )
}

fn browser_access_status_label(status: &BrowserAccessStatus) -> &'static str {
    match status {
        BrowserAccessStatus::Pending => "pending",
        BrowserAccessStatus::Approved => "approved",
        BrowserAccessStatus::Denied => "denied",
        BrowserAccessStatus::Expired => "expired",
    }
}

fn flock_exclusive(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    unsafe {
        if libc::flock(file.as_raw_fd(), libc::LOCK_EX) != 0 {
            anyhow::bail!("failed to lock group chat state")
        }
    }
    Ok(())
}

fn unlock_file(file: &fs::File) -> anyhow::Result<()> {
    #[cfg(unix)]
    unsafe {
        if libc::flock(file.as_raw_fd(), libc::LOCK_UN) != 0 {
            anyhow::bail!("failed to unlock group chat state")
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser_request(
        display_name: &str,
        device_label: &str,
        host_member_did: Option<&str>,
    ) -> BrowserAccessRequestInput {
        BrowserAccessRequestInput {
            display_name: display_name.to_string(),
            device_label: device_label.to_string(),
            host_member_did: host_member_did.map(str::to_string),
            capabilities: room_access_capabilities(),
        }
    }

    #[test]
    fn browser_access_request_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.pending_count, 1);
        let status = browser_access_status(tmp.path(), &request.request_id).unwrap();
        assert_eq!(status.status, "pending");
    }

    #[test]
    fn summary_surfaces_same_pending_request_that_approve_will_use() {
        let tmp = tempfile::tempdir().unwrap();
        let first =
            request_browser_access(tmp.path(), browser_request("Alice", "Phone", None)).unwrap();
        let _second =
            request_browser_access(tmp.path(), browser_request("Bob", "Safari", None)).unwrap();

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.latest_request_name.as_deref(), Some("Alice"));
        assert_eq!(summary.latest_request_device.as_deref(), Some("Phone"));

        let approved = approve_next_request(tmp.path()).unwrap().unwrap();
        assert_eq!(approved.request_id, first.request_id);
        assert_eq!(approved.display_name, "Alice");
    }

    #[test]
    fn approve_request_targets_specific_pending_request() {
        let tmp = tempfile::tempdir().unwrap();
        let first =
            request_browser_access(tmp.path(), browser_request("Alice", "Phone", None)).unwrap();
        let second =
            request_browser_access(tmp.path(), browser_request("Bob", "Safari", None)).unwrap();

        let approved = approve_request(tmp.path(), &second.request_id)
            .unwrap()
            .unwrap();
        assert_eq!(approved.request_id, second.request_id);
        let first_status = browser_access_status(tmp.path(), &first.request_id).unwrap();
        assert_eq!(first_status.status, "pending");
    }

    #[test]
    fn revoke_session_targets_one_browser_only() {
        let tmp = tempfile::tempdir().unwrap();
        let first =
            request_browser_access(tmp.path(), browser_request("Alice", "Phone", None)).unwrap();
        let second =
            request_browser_access(tmp.path(), browser_request("Bob", "Safari", None)).unwrap();
        let _ = approve_request(tmp.path(), &first.request_id)
            .unwrap()
            .unwrap();
        let _ = approve_request(tmp.path(), &second.request_id)
            .unwrap()
            .unwrap();
        let first_token = browser_access_status(tmp.path(), &first.request_id)
            .unwrap()
            .token
            .unwrap();
        let second_token = browser_access_status(tmp.path(), &second.request_id)
            .unwrap()
            .token
            .unwrap();

        let revoked = revoke_session(tmp.path(), &first_token).unwrap().unwrap();
        assert_eq!(revoked.display_name, "Alice");
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_session_count, 1);
        assert_eq!(summary.active_sessions[0].display_name, "Bob");
        assert_eq!(
            browser_access_status(tmp.path(), &first.request_id)
                .unwrap()
                .status,
            "expired"
        );
        assert_eq!(
            browser_access_status(tmp.path(), &second.request_id)
                .unwrap()
                .token,
            Some(second_token)
        );
    }

    #[test]
    fn revoke_guest_session_by_public_id_never_revokes_runtime_nodes() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "Phone", None)).unwrap();
        let _ = approve_request(tmp.path(), &request.request_id)
            .unwrap()
            .unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        let guest_session_id = summary.active_sessions[0].session_id.clone();

        let revoked = revoke_guest_session_by_id(tmp.path(), &guest_session_id)
            .unwrap()
            .unwrap();
        assert_eq!(revoked.display_name, "Alice");
        assert!(load_summary(tmp.path()).unwrap().active_sessions.is_empty());

        let (_, did) = elastos_identity::load_or_create_did(tmp.path()).unwrap();
        let _ = start_local_runtime_session(tmp.path(), &did, "Local runtime", "ElastOS shell")
            .unwrap();
        let summary = load_summary(tmp.path()).unwrap();
        let runtime_session_id = summary.active_sessions[0].session_id.clone();
        let err = revoke_guest_session_by_id(tmp.path(), &runtime_session_id).unwrap_err();
        assert!(err
            .to_string()
            .contains("runtime node sessions must be blocked"));
    }

    #[test]
    fn storage_uses_localhost_root_documents() {
        let tmp = tempfile::tempdir().unwrap();
        request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();

        let paths = storage_paths(tmp.path()).unwrap();
        assert!(paths
            .root_dir
            .ends_with("Local/Shared/AppCapsules/chat-room"));
        assert!(paths
            .room_dir
            .ends_with("Local/Shared/AppCapsules/chat-room/room"));
        assert!(paths
            .local_dir
            .ends_with("Local/Shared/AppCapsules/chat-room/local"));
        assert!(paths.room_meta_path.is_file());
        assert!(paths.control_path.is_file());
        assert!(paths.members_path.is_file());
        assert!(paths.invites_path.is_file());
        assert!(paths.key_epochs_path.is_file());
        assert!(paths.pair_requests_path.is_file());
        assert!(paths.sessions_path.is_file());
        assert!(paths.objects_path.is_file());
    }

    #[test]
    fn seeding_owner_populates_room_control() {
        let tmp = tempfile::tempdir().unwrap();
        let control = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        assert_eq!(control.title, "Exec Room");
        assert_eq!(control.owner_did.as_deref(), Some("did:key:z6owner"));
        assert_eq!(control.current_key_epoch, 1);
        assert_eq!(control.members.len(), 1);
        assert_eq!(control.members[0].role, RoomRole::Owner);
        assert!(control.access_policy.allow_guest_invites);
        assert!(control.access_policy.allow_member_invites);
        assert!(control.access_policy.allow_members_to_host_guests);
    }

    #[test]
    fn owner_can_update_room_access_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();

        let updated = update_room_access_policy(
            tmp.path(),
            RoomAccessPolicyUpdateInput {
                actor_did: "did:key:z6owner".to_string(),
                allow_guest_invites: true,
                allow_member_invites: false,
                allow_members_to_host_guests: false,
            },
        )
        .unwrap();
        assert!(updated.allow_guest_invites);
        assert!(!updated.allow_member_invites);
        assert!(!updated.allow_members_to_host_guests);

        let control = load_room_control(tmp.path()).unwrap();
        assert!(!control.access_policy.allow_member_invites);
        assert!(!control.access_policy.allow_members_to_host_guests);
    }

    #[test]
    fn sovereign_member_invites_respect_room_access_policy() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let _ = update_room_access_policy(
            tmp.path(),
            RoomAccessPolicyUpdateInput {
                actor_did: "did:key:z6owner".to_string(),
                allow_guest_invites: true,
                allow_member_invites: false,
                allow_members_to_host_guests: true,
            },
        )
        .unwrap();

        let err = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("ElastOS user invites are disabled"));
    }

    #[test]
    fn admin_can_invite_member_but_not_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let admin_invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6admin".to_string(),
                role: RoomRole::Admin,
            },
        )
        .unwrap();
        let _admin = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6admin".to_string(),
                invite_id: admin_invite.invite_id,
            },
        )
        .unwrap();

        let member_invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6admin".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        assert_eq!(member_invite.role, RoomRole::Member);

        let err = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6admin".to_string(),
                invited_did: "did:key:z6admin2".to_string(),
                role: RoomRole::Admin,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("admin cannot invite Admin"));
    }

    #[test]
    fn membership_changes_rotate_key_epochs() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        let _member = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6member".to_string(),
                invite_id: invite.invite_id,
            },
        )
        .unwrap();
        let control_after_join = load_room_control(tmp.path()).unwrap();
        assert_eq!(control_after_join.current_key_epoch, 2);

        let removed = remove_room_member(
            tmp.path(),
            RoomMemberRemoveInput {
                actor_did: "did:key:z6owner".to_string(),
                member_did: "did:key:z6member".to_string(),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(removed.member_did, "did:key:z6member");

        let control_after_remove = load_room_control(tmp.path()).unwrap();
        assert_eq!(control_after_remove.current_key_epoch, 3);
        assert_eq!(control_after_remove.members.len(), 1);
        assert_eq!(
            control_after_remove.members[0].member_did,
            "did:key:z6owner"
        );
    }

    #[test]
    fn signed_room_invite_envelope_round_trip_imports_on_another_runtime() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let (_source_sk, source_did) = elastos_identity::load_or_create_did(source.path()).unwrap();
        let (_target_sk, target_did) = elastos_identity::load_or_create_did(target.path()).unwrap();

        let _ = seed_room_owner(
            source.path(),
            RoomOwnerSeedInput {
                owner_did: source_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();

        let envelope = export_room_invite_envelope(
            source.path(),
            RoomInviteInput {
                actor_did: source_did.clone(),
                invited_did: target_did.clone(),
                role: RoomRole::Member,
            },
        )
        .unwrap();

        let imported =
            import_room_invite_envelope(target.path(), &serde_json::to_vec(&envelope).unwrap())
                .unwrap();
        assert_eq!(imported.invited_did, target_did);
        assert_eq!(imported.invited_by, source_did);

        let control = load_room_control(target.path()).unwrap();
        assert_eq!(
            control.owner_did.as_deref(),
            Some(envelope.payload.owner_did.as_deref().unwrap())
        );
        assert_eq!(control.title, "Exec Room");
        assert_eq!(control.pending_invites.len(), 1);
    }

    #[test]
    fn signed_room_acceptance_envelope_round_trip_syncs_owner_runtime() {
        let owner = tempfile::tempdir().unwrap();
        let guest = tempfile::tempdir().unwrap();
        let (_owner_sk, owner_did) = elastos_identity::load_or_create_did(owner.path()).unwrap();
        let (_guest_sk, guest_did) = elastos_identity::load_or_create_did(guest.path()).unwrap();
        let _ = seed_room_owner(
            owner.path(),
            RoomOwnerSeedInput {
                owner_did: owner_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();

        let invite = export_room_invite_envelope(
            owner.path(),
            RoomInviteInput {
                actor_did: owner_did.clone(),
                invited_did: guest_did.clone(),
                role: RoomRole::Member,
            },
        )
        .unwrap();

        let imported_invite =
            import_room_invite_envelope(guest.path(), &serde_json::to_vec(&invite).unwrap())
                .unwrap();
        let accepted = accept_room_invite(
            guest.path(),
            RoomInviteAcceptInput {
                actor_did: guest_did.clone(),
                invite_id: imported_invite.invite_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(accepted.member_did, guest_did);

        let acceptance =
            export_room_acceptance_envelope(guest.path(), &imported_invite.invite_id).unwrap();
        let synced = import_room_acceptance_envelope(
            owner.path(),
            &serde_json::to_vec(&acceptance).unwrap(),
        )
        .unwrap();
        assert_eq!(synced.member_did, guest_did);

        let control = load_room_control(owner.path()).unwrap();
        assert!(control.pending_invites.is_empty());
        assert_eq!(control.members.len(), 2);
        assert_eq!(control.title, "Exec Room");
        assert_eq!(control.active_member_count, 2);
        assert!(control
            .members
            .iter()
            .any(|member| member.member_did == owner_did && member.role == RoomRole::Owner));
        assert!(control
            .members
            .iter()
            .any(|member| member.member_did == guest_did && member.role == RoomRole::Member));
    }

    #[test]
    fn conversation_join_link_claims_targeted_invite_and_syncs_acceptance() {
        let owner = tempfile::tempdir().unwrap();
        let guest = tempfile::tempdir().unwrap();
        let (_owner_sk, owner_did) = elastos_identity::load_or_create_did(owner.path()).unwrap();
        let (_guest_sk, guest_did) = elastos_identity::load_or_create_did(guest.path()).unwrap();
        let owner_profile = RoomProfileCardView {
            schema: "elastos.profile-card/v1".to_string(),
            profile_id: "profile:local:owner".to_string(),
            display_name: "Owner".to_string(),
            handle: Some("owner".to_string()),
            updated_at: 10,
        };
        let guest_profile = RoomProfileCardView {
            schema: "elastos.profile-card/v1".to_string(),
            profile_id: "profile:local:guest".to_string(),
            display_name: "Guest".to_string(),
            handle: Some("guest".to_string()),
            updated_at: 20,
        };

        let _ = seed_room_owner(
            owner.path(),
            RoomOwnerSeedInput {
                owner_did: owner_did.clone(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();

        let join = export_room_join_invite(
            owner.path(),
            RoomJoinInviteInput {
                actor_did: owner_did.clone(),
                issuer_gateway: "https://elastos.example".to_string(),
                inviter_profile: Some(owner_profile.clone()),
            },
        )
        .unwrap();
        assert_eq!(
            room_join_invite_token_from_input(&join.invite_url).unwrap(),
            join.token
        );
        let chat_uri_err =
            room_join_invite_token_from_input(&format!("elastos://chat/join?token={}", join.token))
                .unwrap_err();
        assert!(chat_uri_err
            .to_string()
            .contains("unsupported conversation join invite URI"));
        let (decoded, signer_did) = decode_room_join_invite_token(&join.token).unwrap();
        assert_eq!(signer_did, owner_did);
        assert_eq!(decoded.payload.issuer_gateway, "https://elastos.example");
        assert_eq!(decoded.payload.role, RoomRole::Member);
        assert_eq!(
            decoded
                .payload
                .inviter_profile
                .as_ref()
                .unwrap()
                .display_name,
            "Owner"
        );

        let invite = claim_room_join_invite(owner.path(), &join.invite_url, &guest_did).unwrap();
        assert_eq!(invite.payload.invited_did, guest_did);
        assert_eq!(invite.payload.invited_by, owner_did);
        assert_eq!(
            invite
                .payload
                .inviter_profile
                .as_ref()
                .unwrap()
                .display_name,
            "Owner"
        );

        let imported_invite =
            import_room_invite_envelope(guest.path(), &serde_json::to_vec(&invite).unwrap())
                .unwrap();
        let guest_control_after_import = load_room_control(guest.path()).unwrap();
        assert!(guest_control_after_import.members.iter().any(|member| {
            member.member_did == owner_did
                && member
                    .profile_card
                    .as_ref()
                    .map(|card| card.display_name.as_str())
                    == Some("Owner")
        }));
        let accepted = accept_room_invite(
            guest.path(),
            RoomInviteAcceptInput {
                actor_did: guest_did.clone(),
                invite_id: imported_invite.invite_id.clone(),
            },
        )
        .unwrap();
        assert_eq!(accepted.member_did, guest_did);

        let acceptance = export_room_acceptance_envelope_with_profile(
            guest.path(),
            &imported_invite.invite_id,
            Some(guest_profile),
        )
        .unwrap();
        let synced = import_room_acceptance_envelope(
            owner.path(),
            &serde_json::to_vec(&acceptance).unwrap(),
        )
        .unwrap();
        assert_eq!(synced.member_did, guest_did);
        assert_eq!(
            synced.profile_card.as_ref().unwrap().display_name.as_str(),
            "Guest"
        );

        let control = load_room_control(owner.path()).unwrap();
        assert!(control.pending_invites.is_empty());
        assert!(control
            .members
            .iter()
            .any(|member| member.member_did == owner_did && member.role == RoomRole::Owner));
        assert!(control.members.iter().any(|member| {
            member.member_did == guest_did
                && member.role == RoomRole::Member
                && member
                    .profile_card
                    .as_ref()
                    .map(|card| card.display_name.as_str())
                    == Some("Guest")
        }));
    }

    #[test]
    fn room_poll_lists_only_active_sovereign_participants() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6guest".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        let _ = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6guest".to_string(),
                invite_id: invite.invite_id,
            },
        )
        .unwrap();

        let session =
            start_local_runtime_session(tmp.path(), "did:key:z6owner", "wsl", "laptop").unwrap();

        let poll = room_poll(tmp.path(), &session.token, 0).unwrap();
        assert_eq!(poll.participants.len(), 1);
        assert!(poll.participants.iter().any(|participant| {
            participant.member_did.as_deref() == Some("did:key:z6owner")
                && participant.display_name == "wsl"
                && participant.local_session_count == 1
        }));
        assert!(poll
            .participants
            .iter()
            .all(|participant| participant.member_did.as_deref() != Some("did:key:z6guest")));
    }

    #[test]
    fn owner_can_revoke_pending_member_invite() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();

        let revoked = revoke_room_invite(tmp.path(), "did:key:z6owner", &invite.invite_id)
            .unwrap()
            .expect("invite should revoke");
        assert_eq!(revoked.invite_id, invite.invite_id);

        let control = load_room_control(tmp.path()).unwrap();
        assert!(control.pending_invites.is_empty());
    }

    #[test]
    fn admin_cannot_revoke_pending_admin_invite() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let admin_invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6admin".to_string(),
                role: RoomRole::Admin,
            },
        )
        .unwrap();
        let _ = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6admin".to_string(),
                invite_id: admin_invite.invite_id,
            },
        )
        .unwrap();
        let pending_admin_invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6admin2".to_string(),
                role: RoomRole::Admin,
            },
        )
        .unwrap();

        let err = revoke_room_invite(
            tmp.path(),
            "did:key:z6admin",
            &pending_admin_invite.invite_id,
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("admin cannot revoke Admin invites"));
    }

    #[test]
    fn seeded_room_requires_active_member_did_for_browser_access() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();

        let missing = request_browser_access(tmp.path(), browser_request("Alice", "Phone", None))
            .unwrap_err();
        assert!(missing
            .to_string()
            .contains("no active room member DID available"));

        let non_member = request_browser_access(
            tmp.path(),
            browser_request("Alice", "Phone", Some("did:key:z6stranger")),
        )
        .unwrap_err();
        assert!(non_member
            .to_string()
            .contains("not part of this conversation"));

        let allowed = request_browser_access(
            tmp.path(),
            browser_request("Alice", "Phone", Some("did:key:z6owner")),
        )
        .unwrap();
        assert!(!allowed.request_id.is_empty());
    }

    #[test]
    fn members_can_be_blocked_from_hosting_guest_browsers() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        let _ = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6member".to_string(),
                invite_id: invite.invite_id,
            },
        )
        .unwrap();
        let _ = update_room_access_policy(
            tmp.path(),
            RoomAccessPolicyUpdateInput {
                actor_did: "did:key:z6owner".to_string(),
                allow_guest_invites: true,
                allow_member_invites: true,
                allow_members_to_host_guests: false,
            },
        )
        .unwrap();

        let err = request_browser_access(
            tmp.path(),
            browser_request("Alice", "Phone", Some("did:key:z6member")),
        )
        .unwrap_err();
        assert!(err
            .to_string()
            .contains("only lets managers approve web guests"));
    }

    #[test]
    fn approval_creates_session_and_object_flow() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let status = browser_access_status(tmp.path(), &request.request_id).unwrap();
        let token = status.token.unwrap();
        assert_eq!(status.status, "approved");
        assert_eq!(status.expires_at, Some(approved.expires_at));

        let session = session_view(tmp.path(), &token).unwrap();
        assert_eq!(session.display_name, "Alice");
        assert_eq!(session.participants.len(), 1);
        assert_eq!(session.participants[0].display_name, "Alice");
        let joined = conversation_feed(tmp.path(), &token, 0).unwrap();
        assert_eq!(joined.objects.len(), 1);
        assert_eq!(joined.objects[0].kind, ConversationObjectKind::System);
        assert_eq!(joined.objects[0].body.as_deref(), Some("joined the room"));

        let sent = append_object(tmp.path(), &token, "hello world").unwrap();
        assert_eq!(sent.seq, 2);
        assert_eq!(sent.kind, ConversationObjectKind::Text);
        let feed = conversation_feed(tmp.path(), &token, 0).unwrap();
        assert_eq!(feed.objects.len(), 2);
        assert_eq!(feed.objects[1].body.as_deref(), Some("hello world"));
    }

    #[test]
    fn open_room_allows_local_runtime_session_without_active_membership() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(tmp.path()).unwrap();

        let session =
            start_local_runtime_session(tmp.path(), &did, "Local runtime", "ElastOS shell")
                .unwrap();
        let session_view = session_view(tmp.path(), &session.token).unwrap();
        assert_eq!(session_view.display_name, "Local runtime");
        assert_eq!(session_view.participants.len(), 1);
        assert_eq!(session_view.participants[0].display_name, "Local runtime");
        assert_eq!(
            session_view.participants[0].member_did.as_deref(),
            Some(did.as_str())
        );

        let joined = conversation_feed(tmp.path(), &session.token, 0).unwrap();
        assert_eq!(joined.objects.len(), 1);
        assert_eq!(joined.objects[0].kind, ConversationObjectKind::System);
        assert_eq!(joined.objects[0].body.as_deref(), Some("joined the room"));
        assert_eq!(
            joined.objects[0].sender_member_did.as_deref(),
            Some(did.as_str())
        );
    }

    #[test]
    fn local_runtime_session_handle_change_reuses_single_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(tmp.path()).unwrap();

        let first = start_local_runtime_session(tmp.path(), &did, "Local runtime", "ElastOS shell")
            .unwrap();
        let second =
            start_local_runtime_session(tmp.path(), &did, "anders", "ElastOS shell").unwrap();

        assert_eq!(first.token, second.token);
        assert_eq!(second.display_name, "anders");

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_session_count, 1);
        assert_eq!(summary.active_participants.len(), 1);
        assert_eq!(summary.active_participants[0].display_name, "anders");
        assert_eq!(
            summary.active_participants[0].member_did.as_deref(),
            Some(did.as_str())
        );
    }

    #[test]
    fn guest_poll_does_not_claim_local_runtime_messages_as_current_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(tmp.path()).unwrap();

        let local =
            start_local_runtime_session(tmp.path(), &did, "anders", "ElastOS shell").unwrap();
        let _ = append_object(tmp.path(), &local.token, "hello from shell").unwrap();

        let request =
            request_browser_access(tmp.path(), browser_request("Guest", "Browser", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let guest_token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let poll = room_poll(tmp.path(), &guest_token, 0).unwrap();
        assert!(poll.participants.iter().any(
            |participant| participant.display_name == "Guest" && participant.is_current_session
        ));
        assert!(poll
            .participants
            .iter()
            .any(|participant| participant.display_name == "anders"
                && !participant.is_current_session));
        assert!(poll
            .objects
            .iter()
            .any(|object| object.body.as_deref() == Some("hello from shell")
                && !object.from_current_session));
    }

    #[test]
    fn local_runtime_poll_marks_runtime_messages_as_current_session() {
        let tmp = tempfile::tempdir().unwrap();
        let (_, did) = elastos_identity::load_or_create_did(tmp.path()).unwrap();

        let local =
            start_local_runtime_session(tmp.path(), &did, "anders", "ElastOS shell").unwrap();
        let sent = append_object(tmp.path(), &local.token, "hello from shell").unwrap();
        assert!(sent.from_current_session);

        let poll = room_poll(tmp.path(), &local.token, 0).unwrap();
        assert!(poll
            .participants
            .iter()
            .any(|participant| participant.display_name == "anders"
                && participant.is_current_session));
        assert!(poll
            .objects
            .iter()
            .any(|object| object.body.as_deref() == Some("hello from shell")
                && object.from_current_session));
    }

    #[test]
    fn same_runtime_passkey_principals_share_one_member_roster_row() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime_did = "did:key:z6runtime";

        let admin = start_local_principal_runtime_session(
            tmp.path(),
            runtime_did,
            "person:local:admin",
            "Admin",
            "ElastOS shell",
        )
        .unwrap();
        let guest = start_local_principal_runtime_session(
            tmp.path(),
            runtime_did,
            "person:local:guest",
            "Guest",
            "ElastOS shell",
        )
        .unwrap();

        assert_ne!(admin.token, guest.token);
        let _ = append_object(tmp.path(), &admin.token, "admin hello").unwrap();
        let _ = append_object(tmp.path(), &guest.token, "guest hello").unwrap();

        let admin_poll = room_poll(tmp.path(), &admin.token, 0).unwrap();
        let guest_poll = room_poll(tmp.path(), &guest.token, 0).unwrap();

        assert_eq!(admin_poll.participants.len(), 1);
        assert_eq!(guest_poll.participants.len(), 1);
        assert_eq!(
            admin_poll.participants[0].member_did.as_deref(),
            Some(runtime_did)
        );
        assert_eq!(admin_poll.participants[0].local_session_count, 2);
        assert!(admin_poll.participants[0].is_current_session);
        assert!(guest_poll.participants[0].is_current_session);
        assert!(admin_poll.objects.iter().any(|object| {
            object.body.as_deref() == Some("admin hello") && object.from_current_session
        }));
        assert!(admin_poll.objects.iter().any(|object| {
            object.body.as_deref() == Some("guest hello") && !object.from_current_session
        }));
        assert!(guest_poll.objects.iter().any(|object| {
            object.body.as_deref() == Some("admin hello") && !object.from_current_session
        }));
        assert!(guest_poll.objects.iter().any(|object| {
            object.body.as_deref() == Some("guest hello") && object.from_current_session
        }));
    }

    #[test]
    fn stale_principal_actor_objects_collapse_to_one_member_roster_row() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime_did = "did:key:z6runtime";

        with_locked_state(tmp.path(), |_, state| {
            state.members.push(RoomMemberRecord {
                member_did: runtime_did.to_string(),
                role: RoomRole::Owner,
                added_at: 1,
                added_by: runtime_did.to_string(),
                active: true,
                profile_card: None,
                removed_at: None,
                removed_by: None,
            });
            state.objects.push(ConversationObjectRecord {
                seq: 1,
                event_id: "evt-admin".to_string(),
                sender: "Admin".to_string(),
                sender_member_did: Some(runtime_did.to_string()),
                sender_actor_id: "principal:admin".to_string(),
                kind: ConversationObjectKind::Text,
                body: Some("admin hello".to_string()),
                emoji: None,
                link: None,
                attachment: None,
                created_at: 10,
            });
            state.objects.push(ConversationObjectRecord {
                seq: 2,
                event_id: "evt-guest".to_string(),
                sender: "Guest".to_string(),
                sender_member_did: Some(runtime_did.to_string()),
                sender_actor_id: "principal:guest".to_string(),
                kind: ConversationObjectKind::Text,
                body: Some("guest hello".to_string()),
                emoji: None,
                link: None,
                attachment: None,
                created_at: 20,
            });
            Ok(())
        })
        .unwrap();

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_participants.len(), 1);
        assert_eq!(
            summary.active_participants[0].member_did.as_deref(),
            Some(runtime_did)
        );
        assert_eq!(summary.active_participants[0].display_name, "Guest");
    }

    #[test]
    fn passkey_principal_participant_replaces_legacy_runtime_actor_row() {
        let tmp = tempfile::tempdir().unwrap();
        let runtime_did = "did:key:z6runtime";

        let legacy =
            start_local_runtime_session(tmp.path(), runtime_did, "Legacy", "ElastOS shell")
                .unwrap();
        let _ = append_object(tmp.path(), &legacy.token, "legacy hello").unwrap();

        let admin = start_local_principal_runtime_session(
            tmp.path(),
            runtime_did,
            "person:local:admin",
            "Admin",
            "ElastOS shell",
        )
        .unwrap();
        let guest = start_local_principal_runtime_session(
            tmp.path(),
            runtime_did,
            "person:local:guest",
            "Guest",
            "ElastOS shell",
        )
        .unwrap();

        let poll = room_poll(tmp.path(), &admin.token, 0).unwrap();

        assert!(room_poll(tmp.path(), &legacy.token, 0).is_err());
        assert_eq!(poll.participants.len(), 1);
        assert_eq!(
            poll.participants[0].member_did.as_deref(),
            Some(runtime_did)
        );
        assert_eq!(poll.participants[0].local_session_count, 2);
        assert!(poll.participants[0].is_current_session);
        assert!(!poll
            .participants
            .iter()
            .any(|participant| participant.display_name == "Legacy"));
        assert!(poll.objects.iter().any(|object| {
            object.body.as_deref() == Some("legacy hello") && !object.from_current_session
        }));

        let guest_poll = room_poll(tmp.path(), &guest.token, 0).unwrap();
        assert_eq!(guest_poll.participants.len(), 1);
        assert!(guest_poll.participants[0].is_current_session);
    }

    #[test]
    fn member_browser_access_does_not_emit_duplicate_join_system_object() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let request = request_browser_access(
            tmp.path(),
            browser_request("Alice", "Laptop", Some("did:key:z6owner")),
        )
        .unwrap();
        let _ = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let poll = room_poll(tmp.path(), &token, 0).unwrap();
        assert!(!poll.objects.iter().any(|object| {
            object.kind == ConversationObjectKind::System
                && object.sender_member_did.as_deref() == Some("did:key:z6owner")
                && object.body.as_deref() == Some("joined the room")
        }));
    }

    #[test]
    fn hosted_browser_guest_does_not_inherit_host_member_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let request = request_browser_access(
            tmp.path(),
            browser_request("Guest", "Browser", Some("did:key:z6owner")),
        )
        .unwrap();
        let _ = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let appended = append_object_with_transport(tmp.path(), &token, "guest hello").unwrap();
        assert!(appended.sender_member_did.is_none());
        assert!(appended.transport_envelope.is_none());

        let poll = room_poll(tmp.path(), &token, 0).unwrap();
        assert!(poll.participants.iter().any(|participant| {
            participant.display_name == "Guest"
                && participant.member_did.is_none()
                && participant.is_current_session
        }));
    }

    #[test]
    fn append_object_with_transport_emits_member_signed_envelope() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let session =
            start_local_runtime_session(tmp.path(), "did:key:z6owner", "Alice", "Phone").unwrap();

        let appended =
            append_object_with_transport(tmp.path(), &session.token, "hello world").unwrap();
        assert_eq!(
            appended.sender_member_did.as_deref(),
            Some("did:key:z6owner")
        );
        let envelope = appended.transport_envelope.unwrap();
        assert_eq!(envelope.schema, ROOM_OBJECT_ENVELOPE_SCHEMA);
        assert_eq!(envelope.room_slug, ROOM_SLUG);
        assert_eq!(envelope.sender_member_did, "did:key:z6owner");
        assert_eq!(envelope.kind, ConversationObjectKind::Text);
        assert_eq!(envelope.body.as_deref(), Some("hello world"));
        assert!(!envelope.event_id.is_empty());
    }

    #[test]
    fn open_room_ingests_transport_objects_without_preseeded_membership() {
        let tmp = tempfile::tempdir().unwrap();
        let envelope = RoomObjectEnvelope {
            schema: ROOM_OBJECT_ENVELOPE_SCHEMA.to_string(),
            room_slug: ROOM_SLUG.to_string(),
            event_id: "event-open-room".to_string(),
            sender: "Remote runtime".to_string(),
            sender_member_did: "did:key:z6remote".to_string(),
            kind: ConversationObjectKind::Text,
            body: Some("global hello".to_string()),
            emoji: None,
            link: None,
            attachment: None,
            attachment_bytes_b64: None,
            created_at: now_ts(),
        };

        let first = ingest_room_object_envelope(tmp.path(), &envelope)
            .unwrap()
            .unwrap();
        assert_eq!(first.body.as_deref(), Some("global hello"));
        assert_eq!(first.sender_member_did.as_deref(), Some("did:key:z6remote"));
        let second = ingest_room_object_envelope(tmp.path(), &envelope).unwrap();
        assert!(second.is_none());
    }

    #[test]
    fn ingest_room_object_envelope_is_idempotent_for_active_members() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        let _ = accept_room_invite(
            tmp.path(),
            RoomInviteAcceptInput {
                actor_did: "did:key:z6member".to_string(),
                invite_id: invite.invite_id,
            },
        )
        .unwrap();

        let envelope = RoomObjectEnvelope {
            schema: ROOM_OBJECT_ENVELOPE_SCHEMA.to_string(),
            room_slug: ROOM_SLUG.to_string(),
            event_id: "event-123".to_string(),
            sender: "Bob".to_string(),
            sender_member_did: "did:key:z6member".to_string(),
            kind: ConversationObjectKind::Text,
            body: Some("remote hello".to_string()),
            emoji: None,
            link: None,
            attachment: None,
            attachment_bytes_b64: None,
            created_at: now_ts(),
        };

        let first = ingest_room_object_envelope(tmp.path(), &envelope)
            .unwrap()
            .unwrap();
        assert_eq!(first.kind, ConversationObjectKind::Text);
        assert_eq!(first.body.as_deref(), Some("remote hello"));
        let second = ingest_room_object_envelope(tmp.path(), &envelope).unwrap();
        assert!(second.is_none());

        let objects: Vec<ConversationObjectRecord> =
            read_json_or_default(&storage_paths(tmp.path()).unwrap().objects_path).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].event_id, "event-123");
    }

    #[test]
    fn ingest_room_object_envelope_rejects_sender_outside_membership() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let envelope = RoomObjectEnvelope {
            schema: ROOM_OBJECT_ENVELOPE_SCHEMA.to_string(),
            room_slug: ROOM_SLUG.to_string(),
            event_id: "event-unauthorized".to_string(),
            sender: "Mallory".to_string(),
            sender_member_did: "did:key:z6mallory".to_string(),
            kind: ConversationObjectKind::Text,
            body: Some("hi".to_string()),
            emoji: None,
            link: None,
            attachment: None,
            attachment_bytes_b64: None,
            created_at: now_ts(),
        };

        let err = ingest_room_object_envelope(tmp.path(), &envelope).unwrap_err();
        assert!(err
            .to_string()
            .contains("sender member DID is not active in this room"));
    }

    #[test]
    fn classify_link_and_emoji_objects() {
        let link = classify_object_body("https://elastos.net/path").unwrap();
        assert_eq!(link.kind, ConversationObjectKind::Link);
        assert_eq!(link.link.unwrap().host, "elastos.net");

        let document = classify_object_body(
            "elastos://bafybeigdyrzt5sfp7udm7hu76uh7y26nf3efuylqabf3oclgtqy55fbzdi",
        )
        .unwrap();
        assert_eq!(document.kind, ConversationObjectKind::Link);
        let document_link = document.link.unwrap();
        assert_eq!(document_link.host, "Documents");
        assert_eq!(
            document_link.title,
            "Published document / bafybeigdy…5fbzdi"
        );

        let emoji = classify_object_body("🔥").unwrap();
        assert_eq!(emoji.kind, ConversationObjectKind::Emoji);
        assert_eq!(emoji.emoji.as_deref(), Some("🔥"));
    }

    #[test]
    fn attachment_object_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let sent =
            append_attachment_object(tmp.path(), &token, "photo.png", "image/png", b"png-data")
                .unwrap();
        assert_eq!(sent.kind, ConversationObjectKind::Attachment);
        assert!(sent.attachment.as_ref().unwrap().is_image);

        let feed = conversation_feed(tmp.path(), &token, 0).unwrap();
        assert_eq!(feed.objects.len(), 2);
        let attachment = feed.objects[1].attachment.as_ref().unwrap();
        let (meta, bytes) = read_attachment(tmp.path(), &token, &attachment.attachment_id).unwrap();
        assert_eq!(meta.file_name, "photo.png");
        assert_eq!(bytes, b"png-data");
        assert!(!meta.is_audio);
        assert!(!meta.is_video);
    }

    #[test]
    fn attachment_upload_finish_preserves_retry_state_when_incomplete() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let upload =
            start_attachment_upload(tmp.path(), &token, "draft.txt", "text/plain", 5).unwrap();
        append_attachment_upload_chunk(tmp.path(), &token, &upload.upload_id, 0, b"he").unwrap();

        let err = finish_attachment_upload(tmp.path(), &token, &upload.upload_id)
            .unwrap_err()
            .to_string();
        assert!(err.contains("upload incomplete"));

        append_attachment_upload_chunk(tmp.path(), &token, &upload.upload_id, 2, b"llo").unwrap();
        let finished = finish_attachment_upload(tmp.path(), &token, &upload.upload_id).unwrap();
        let attachment_id = &finished.object.attachment.as_ref().unwrap().attachment_id;
        let (_meta, bytes) = read_attachment(tmp.path(), &token, attachment_id).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn attachment_upload_finish_preserves_retry_state_when_staged_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let upload =
            start_attachment_upload(tmp.path(), &token, "draft.txt", "text/plain", 4).unwrap();
        append_attachment_upload_chunk(tmp.path(), &token, &upload.upload_id, 0, b"data").unwrap();
        let paths = storage_paths(tmp.path()).unwrap();
        let staged_path = upload_staging_path(&paths, &upload.upload_id);
        fs::remove_file(&staged_path).unwrap();

        let err = finish_attachment_upload(tmp.path(), &token, &upload.upload_id)
            .unwrap_err()
            .to_string();
        assert!(err.contains("failed to read staged upload"));

        fs::write(&staged_path, b"data").unwrap();
        let finished = finish_attachment_upload(tmp.path(), &token, &upload.upload_id).unwrap();
        let attachment_id = &finished.object.attachment.as_ref().unwrap().attachment_id;
        let (_meta, bytes) = read_attachment(tmp.path(), &token, attachment_id).unwrap();
        assert_eq!(bytes, b"data");
    }

    #[cfg(unix)]
    #[test]
    fn attachment_upload_finish_preserves_retry_state_when_final_state_save_fails() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let upload =
            start_attachment_upload(tmp.path(), &token, "draft.txt", "text/plain", 4).unwrap();
        append_attachment_upload_chunk(tmp.path(), &token, &upload.upload_id, 0, b"data").unwrap();
        let paths = storage_paths(tmp.path()).unwrap();
        let staged_path = upload_staging_path(&paths, &upload.upload_id);
        fs::create_dir_all(&paths.attachments_dir).unwrap();
        fs::set_permissions(&paths.attachments_dir, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&paths.room_dir, fs::Permissions::from_mode(0o500)).unwrap();

        let err = finish_attachment_upload(tmp.path(), &token, &upload.upload_id)
            .unwrap_err()
            .to_string();
        fs::set_permissions(&paths.room_dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(err.contains("failed to write") || err.contains("Permission denied"));
        assert!(
            staged_path.exists(),
            "failed final commit must keep staged upload bytes"
        );

        let finished = finish_attachment_upload(tmp.path(), &token, &upload.upload_id).unwrap();
        assert!(
            !staged_path.exists(),
            "successful finish should clean the staged upload file"
        );
        let attachment_id = &finished.object.attachment.as_ref().unwrap().attachment_id;
        let (_meta, bytes) = read_attachment(tmp.path(), &token, attachment_id).unwrap();
        assert_eq!(bytes, b"data");
    }

    #[test]
    fn attachment_object_classifies_audio_and_video() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let audio =
            append_attachment_object(tmp.path(), &token, "voice.ogg", "audio/ogg", b"ogg-data")
                .unwrap();
        let audio_meta = audio.attachment.unwrap();
        assert!(audio_meta.is_audio);
        assert!(!audio_meta.is_video);
        assert!(!audio_meta.is_image);

        let video =
            append_attachment_object(tmp.path(), &token, "clip.mp4", "video/mp4", b"mp4-data")
                .unwrap();
        let video_meta = video.attachment.unwrap();
        assert!(video_meta.is_video);
        assert!(!video_meta.is_audio);
        assert!(!video_meta.is_image);
    }

    #[test]
    fn leave_session_removes_participant_and_appends_system_object() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "iPhone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let left = leave_session(tmp.path(), &token).unwrap();
        assert_eq!(left.kind, ConversationObjectKind::System);
        assert_eq!(left.body.as_deref(), Some("left the room"));

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_session_count, 0);
        let status = browser_access_status(tmp.path(), &request.request_id).unwrap();
        assert_eq!(status.status, "expired");
        assert!(status.token.is_none());
    }

    #[test]
    fn last_member_leave_emits_transportable_system_object() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let session =
            start_local_runtime_session(tmp.path(), "did:key:z6owner", "Alice", "Laptop").unwrap();

        let left = leave_session_with_transport(tmp.path(), &session.token).unwrap();
        assert_eq!(left.object.kind, ConversationObjectKind::System);
        assert_eq!(left.object.body.as_deref(), Some("left the room"));
        assert_eq!(
            left.object.sender_member_did.as_deref(),
            Some("did:key:z6owner")
        );
        let envelope = left.transport_envelope.expect("leave transport envelope");
        assert_eq!(envelope.kind, ConversationObjectKind::System);
        assert_eq!(envelope.sender_member_did, "did:key:z6owner");
        assert_eq!(envelope.body.as_deref(), Some("left the room"));

        let summary = load_summary(tmp.path()).unwrap();
        assert!(summary.active_participants.is_empty());
    }

    #[test]
    fn deny_marks_request_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Bob", "Safari", None)).unwrap();
        let denied = deny_next_request(tmp.path(), "No").unwrap().unwrap();
        assert_eq!(denied.display_name, "Bob");
        let status = browser_access_status(tmp.path(), &request.request_id).unwrap();
        assert_eq!(status.status, "denied");
        assert_eq!(status.denial_reason.as_deref(), Some("No"));
    }

    #[test]
    fn summary_includes_active_participants() {
        let tmp = tempfile::tempdir().unwrap();
        let request =
            request_browser_access(tmp.path(), browser_request("Alice", "Phone", None)).unwrap();
        let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
        let _token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_session_count, 1);
        assert_eq!(summary.active_participants.len(), 1);
        assert_eq!(summary.active_participants[0].display_name, "Alice");
    }

    #[test]
    fn revoke_all_sessions_clears_room_and_appends_system_objects() {
        let tmp = tempfile::tempdir().unwrap();
        let mut request_ids = Vec::new();
        for (display_name, device_label) in [("Alice", "Phone"), ("Bob", "Safari")] {
            let request = request_browser_access(
                tmp.path(),
                browser_request(display_name, device_label, None),
            )
            .unwrap();
            request_ids.push(request.request_id.clone());
            let _approved = approve_next_request(tmp.path()).unwrap().unwrap();
            let _token = browser_access_status(tmp.path(), &request.request_id)
                .unwrap()
                .token
                .unwrap();
        }

        let revoked = revoke_all_sessions(tmp.path()).unwrap().unwrap();
        assert_eq!(revoked.revoked_count, 2);
        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.active_session_count, 0);
        let first_status = browser_access_status(tmp.path(), &request_ids[0]).unwrap();
        assert_eq!(first_status.status, "expired");
        assert!(first_status.token.is_none());
        let second_status = browser_access_status(tmp.path(), &request_ids[1]).unwrap();
        assert_eq!(second_status.status, "expired");
        assert!(second_status.token.is_none());

        let feed = conversation_feed(tmp.path(), "invalid", 0);
        assert!(feed.is_err());
        let objects: Vec<_> = read_json_or_default::<Vec<ConversationObjectRecord>>(
            &storage_paths(tmp.path()).unwrap().objects_path,
        )
        .unwrap();
        assert!(objects
            .iter()
            .any(|item| item.body.as_deref() == Some("was removed from the room in Home")));
    }

    #[test]
    fn reset_room_clears_live_state_but_preserves_governance() {
        let tmp = tempfile::tempdir().unwrap();
        let _ = seed_room_owner(
            tmp.path(),
            RoomOwnerSeedInput {
                owner_did: "did:key:z6owner".to_string(),
                title: "Exec Room".to_string(),
            },
        )
        .unwrap();
        let invite = invite_room_member(
            tmp.path(),
            RoomInviteInput {
                actor_did: "did:key:z6owner".to_string(),
                invited_did: "did:key:z6member".to_string(),
                role: RoomRole::Member,
            },
        )
        .unwrap();
        let request = request_browser_access(
            tmp.path(),
            browser_request("Alice", "Phone", Some("did:key:z6owner")),
        )
        .unwrap();
        let _ = approve_request(tmp.path(), &request.request_id)
            .unwrap()
            .unwrap();
        let token = browser_access_status(tmp.path(), &request.request_id)
            .unwrap()
            .token
            .unwrap();
        let _ = append_object(tmp.path(), &token, "hello world").unwrap();
        let _ =
            append_attachment_object(tmp.path(), &token, "photo.png", "image/png", b"png").unwrap();
        let _ = start_attachment_upload(tmp.path(), &token, "draft.txt", "text/plain", 4).unwrap();

        let reset = reset_room(tmp.path()).unwrap();
        assert_eq!(reset.room_slug, ROOM_SLUG);
        assert_eq!(reset.cleared_requests, 1);
        assert_eq!(reset.cleared_sessions, 1);
        assert!(reset.cleared_objects >= 3);
        assert_eq!(reset.cleared_uploads, 1);
        assert_eq!(reset.cleared_attachments, 1);

        let summary = load_summary(tmp.path()).unwrap();
        assert_eq!(summary.pending_count, 0);
        assert_eq!(summary.active_session_count, 0);
        assert!(summary.active_participants.is_empty());
        assert!(summary.pending_requests.is_empty());
        assert!(summary.active_sessions.is_empty());
        assert_eq!(
            summary.room_control.owner_did.as_deref(),
            Some("did:key:z6owner")
        );
        assert_eq!(summary.room_control.member_count, 1);
        assert_eq!(summary.room_control.pending_invites.len(), 1);
        assert_eq!(
            summary.room_control.pending_invites[0].invite_id,
            invite.invite_id
        );

        let paths = storage_paths(tmp.path()).unwrap();
        assert!(!paths.attachments_dir.exists());
        assert!(!paths.uploads_dir.exists());

        let meta: RoomMeta = read_json_or_default(&paths.room_meta_path).unwrap();
        assert_eq!(meta.next_seq, 1);
    }
}
