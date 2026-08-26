#[derive(Serialize)]
struct HomeSummaryResponse {
    home: HomeRouteInfo,
    app: HomeCapsuleIdentity,
    identity: HomeIdentitySummary,
    authority: HomeAuthoritySummary,
    browser_state: HomeBrowserStateSummary,
    active_shell: HomeActiveShellSummary,
    appearance: HomeAppearanceSummary,
    runtime: HomeRuntimeSummary,
    site: HomeSiteSummary,
    room: HomeRoomSummary,
    people: HomePeopleSummary,
    services: HomeServicesSummary,
    notifications: HomeNotificationsSummary,
    desktop_objects: HomeDesktopObjectsSummary,
    capsule_catalog: CapsuleCatalogResponse,
    capsule_interfaces: CapsuleInterfaceRegistryResponse,
    targets: Vec<HomeTargetSummary>,
}

#[derive(Serialize)]
struct HomeRouteInfo {
    route: String,
    attach_kind: String,
}

#[derive(Serialize)]
struct HomeIdentitySummary {
    /// The local Runtime device identity. Serialized to exactly one browser
    /// surface: System, the runtime-inspection page. Every other capsule read
    /// model strips it via `without_device_identity` — invariant 1 keeps
    /// device identity out of app-facing projections, and no other surface
    /// consumes it.
    device_did: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_readiness: Option<ProfileReadinessSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    recovery_readiness: Option<RecoveryReadinessSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_setup_display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile: Option<HomeProfileSummary>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProfileReadinessSummary {
    schema: &'static str,
    status: &'static str,
}

impl ProfileReadinessSummary {
    const SCHEMA: &'static str = "elastos.profile.readiness/v1";

    fn ready() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "ready",
        }
    }

    fn setup_required() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "setup_required",
        }
    }

    fn unavailable() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RecoveryReadinessSummary {
    schema: &'static str,
    status: &'static str,
}

impl RecoveryReadinessSummary {
    const SCHEMA: &'static str = "elastos.recovery.readiness/v1";

    fn ready() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "ready",
        }
    }

    fn setup_required() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "setup_required",
        }
    }

    fn unavailable() -> Self {
        Self {
            schema: Self::SCHEMA,
            status: "unavailable",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HomeProfileSummary {
    schema: String,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct HomeProfileCardSummary {
    schema: String,
    profile_id: String,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    updated_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct HomePeopleSummary {
    schema: String,
    contact_count: usize,
    #[serde(default)]
    contacts: Vec<HomePeopleContactSummary>,
    service_offer_count: usize,
    #[serde(default)]
    service_offers: Vec<HomeServiceOfferSummary>,
}

impl Default for HomePeopleSummary {
    fn default() -> Self {
        Self {
            schema: "elastos.people.contacts/v1".to_string(),
            contact_count: 0,
            contacts: Vec::new(),
            service_offer_count: 0,
            service_offers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct HomePeopleContactSummary {
    contact_id: String,
    #[serde(skip)]
    remote_profile_did: Option<String>,
    #[serde(skip)]
    added_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    conversation_id: Option<String>,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    relationship: String,
    can_message: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_card: Option<HomeProfileCardSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<u64>,
    /// Presence-derived: `Some(true)` when the contact's signed Profile has an
    /// unexpired presence heartbeat right now, `Some(false)` when presence is
    /// configured and it does not, `None` when there is no presence basis to
    /// answer from. Heartbeat metadata — deliberately excluded from the People
    /// realtime signature, like `last_seen_at`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reachable: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct HomeServiceOfferSummary {
    schema: String,
    offer_id: String,
    service_uri: String,
    service_kind: String,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_uri: Option<String>,
    provider_label: String,
    policy_summary: String,
    status: String,
    enabled: bool,
    grant_required: bool,
    grant_scope: String,
    capsule_contract: String,
    source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_contract: Option<HomeServiceRuntimeContractSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contact_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capsule_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    route: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HomeServiceRuntimeContractSummary {
    schema: String,
    backing_substrate: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    supported_display_modes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    supported_guarantee_levels: Vec<String>,
    direct_network: bool,
    wallet_injection: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HomeServicesSummary {
    schema: String,
    local_offer_count: usize,
    remote_offer_count: usize,
    available_local_offer_count: usize,
    available_remote_offer_count: usize,
    #[serde(default)]
    local_offers: Vec<HomeServiceOfferSummary>,
    #[serde(default)]
    remote_offers: Vec<HomeServiceOfferSummary>,
    #[serde(default)]
    available_local_offers: Vec<HomeServiceOfferSummary>,
    #[serde(default)]
    available_remote_offers: Vec<HomeServiceOfferSummary>,
    grant_model: String,
    carrier_contract: String,
    capsule_contract: String,
}

impl Default for HomeServicesSummary {
    fn default() -> Self {
        Self {
            schema: "elastos.runtime.services/v1".to_string(),
            local_offer_count: 0,
            remote_offer_count: 0,
            available_local_offer_count: 0,
            available_remote_offer_count: 0,
            local_offers: Vec::new(),
            remote_offers: Vec::new(),
            available_local_offers: Vec::new(),
            available_remote_offers: Vec::new(),
            grant_model: "principal_scoped_provider_grant".to_string(),
            carrier_contract: "People discovers trusted offers; Carrier carries signed offer envelopes; providers enforce grants.".to_string(),
            capsule_contract: "capsule -> runtime capability -> provider grant -> service".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
struct HomeAuthoritySummary {
    signed_in: bool,
    principal_id: String,
    session_id: String,
    #[serde(default)]
    proof_binding_id: Option<String>,
    wallet_connected: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeBrowserStateSummary {
    schema: String,
    principal_id: String,
    localhost_root: String,
    #[serde(default)]
    layout: Option<serde_json::Value>,
    #[serde(default)]
    session: Option<serde_json::Value>,
    #[serde(default)]
    recent_targets: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeBrowserStateUpdate {
    #[serde(default)]
    layout: Option<Option<serde_json::Value>>,
    #[serde(default)]
    session: Option<Option<serde_json::Value>>,
    #[serde(default)]
    recent_targets: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct HomeActiveShellSummary {
    schema: String,
    active: String,
    #[serde(default)]
    candidates: Vec<HomeActiveShellCandidate>,
}

#[derive(Debug, Clone, Serialize)]
struct HomeActiveShellCandidate {
    name: String,
    title: String,
    description: String,
    route: String,
    role: CapsuleRole,
    launchable: bool,
    trust_state: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomeActiveShellState {
    schema: String,
    principal_id: String,
    localhost_root: String,
    active: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeActiveShellUpdate {
    active: String,
}

#[derive(Debug, Clone, Serialize)]
struct HomeDesktopObjectsSummary {
    schema: String,
    uri: String,
    #[serde(default)]
    objects: Vec<serde_json::Value>,
    stale: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeopleProfileUpdateRequest {
    display_name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemBackgroundOverlayRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "home_background_overlay_opacity_default")]
    opacity: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeAppearancePreferencesUpdate {
    #[serde(default)]
    theme: Option<String>,
    #[serde(default)]
    accent: Option<String>,
    #[serde(default)]
    accent_custom: Option<String>,
    #[serde(default)]
    dock_auto_hide: Option<bool>,
    #[serde(default)]
    sounds: Option<bool>,
    #[serde(default)]
    focus_mode: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemGuestRegistrationRequest {
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletApprovalRejectRequest {
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletApprovalApproveRequest {
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    step_up_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletApprovalCompleteRequest {
    payload_hash: String,
    #[serde(default)]
    signature: Option<String>,
    #[serde(default)]
    signature_type: Option<String>,
    #[serde(default)]
    public_key: Option<String>,
    signer: String,
    #[serde(default)]
    transaction_hash: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemWalletManagedCreateRequest {
    #[serde(default)]
    chain_namespace: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    create_new: bool,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemWalletDefaultRequest {
    account_id: String,
    chain_namespace: String,
    intent: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletAccountRenameRequest {
    label: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletAccountDeleteRequest {
    step_up_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletAccountRecoveryKeyRequest {
    step_up_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletAccountImportRecoveryKeyRequest {
    step_up_token: String,
    recovery_key: serde_json::Value,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletSendTransactionRequest {
    account_id: String,
    chain_namespace: String,
    to: String,
    amount: String,
    step_up_token: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletQrRequest {
    address: String,
}

#[derive(Serialize)]
struct WalletQrResponse {
    svg: String,
}

#[derive(Serialize)]
struct HomeCapsuleIdentity {
    id: String,
    route: String,
}

#[derive(Serialize)]
struct SystemSummaryResponse {
    identity: HomeIdentitySummary,
    authority: HomeAuthoritySummary,
    access: SystemAccessSummary,
    home: HomeCapsuleIdentity,
    app: SystemCapsuleIdentity,
    appearance: HomeAppearanceSummary,
    source: SystemSourceSummary,
    runtime: HomeRuntimeSummary,
    wallet_accounts: SystemWalletAccountsSummary,
    wallet_approvals: SystemWalletApprovalsSummary,
    runtime_log: SystemRuntimeLogSummary,
}

#[derive(Serialize)]
struct SystemSourceSummary {
    configured: bool,
    name: Option<String>,
    channel: String,
    installed_version: String,
    runtime_version: String,
    mode: String,
    update_checks_allowed: bool,
    update_policy: String,
    transport: String,
    source_peer: Option<String>,
}

#[derive(Serialize)]
struct SystemCapsuleIdentity {
    id: String,
    route: String,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemAccessSummary {
    role: String,
    #[serde(default)]
    localhost_root: Option<String>,
    guest_registration_enabled: bool,
}

#[derive(Debug, Clone, Serialize)]
struct HomeAppearanceSummary {
    schema: String,
    revision: u64,
    theme: String,
    accent: String,
    accent_custom: String,
    dock_auto_hide: bool,
    sounds: bool,
    focus_mode: bool,
    #[serde(default)]
    background_image_url: Option<String>,
    background_overlay_enabled: bool,
    background_overlay_opacity: f64,
}

#[derive(Serialize)]
struct InboxSummaryResponse {
    app: HomeCapsuleIdentity,
    notifications: HomeNotificationsSummary,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RuntimeCapabilityPendingResponse {
    #[serde(default)]
    requests: Vec<RuntimeCapabilityPendingRequest>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RuntimeCapabilityPendingRequest {
    request_id: String,
    resource: String,
    action: String,
    requested_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemWalletAccountsSummary {
    available: bool,
    linked_count: usize,
    #[serde(default)]
    accounts: Vec<SystemWalletAccountSummary>,
    #[serde(default)]
    default_accounts: Vec<SystemWalletDefaultSummary>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemWalletAccountSummary {
    account_id: String,
    chain_namespace: String,
    address: String,
    proof_type: String,
    signing_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    signing_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connector_id: Option<String>,
    linked_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemWalletDefaultSummary {
    chain_namespace: String,
    intent: String,
    account_id: String,
    set_at: u64,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemWalletApprovalsSummary {
    available: bool,
    pending_count: usize,
    approval_requests: Vec<SystemWalletApprovalSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    handoff: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SystemWalletApprovalSummary {
    request_id: String,
    status: String,
    intent: String,
    capsule_id: String,
    resource: String,
    reason: String,
    account_id: String,
    address: String,
    proof_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    connector_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review: Option<serde_json::Value>,
    created_at: u64,
    expires_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transaction_hash: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct SystemRuntimeLogSummary {
    available: bool,
    #[serde(default)]
    total_in_memory: Option<usize>,
    #[serde(default)]
    current_epoch: Option<u64>,
    #[serde(default)]
    events: Vec<SystemRuntimeEventSummary>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SystemRuntimeEventSummary {
    kind: String,
    #[serde(default)]
    at: Option<u64>,
    summary: String,
}

const SYSTEM_RUNTIME_ACTIVITY_FETCH_LIMIT: usize = 32;
const SYSTEM_RUNTIME_ACTIVITY_DISPLAY_LIMIT: usize = 4;
const HOME_BACKGROUND_IMAGE_MAX_BYTES: usize = 5 * 1024 * 1024;
const HOME_BACKGROUND_IMAGE_TRANSPORT_MAX_BYTES: usize = 8 * 1024 * 1024;
const HOME_BACKGROUND_OVERLAY_FILE: &str = "background-overlay.json";
const HOME_BACKGROUND_OVERLAY_DEFAULT: bool = false;
const HOME_BACKGROUND_OVERLAY_OPACITY_DEFAULT: f64 = 0.55;
const HOME_BACKGROUND_OVERLAY_OPACITY_MAX: f64 = 0.8;
const HOME_BACKGROUND_IMAGE_FILES: &[(&str, &str)] = &[
    ("background-image.png", "image/png"),
    ("background-image.jpg", "image/jpeg"),
    ("background-image.webp", "image/webp"),
    ("background-image.gif", "image/gif"),
];

#[derive(Debug, Clone, Default)]
struct GatewayRuntimeLaunchOutcome {
    status: String,
    capsule_id: Option<String>,
    detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRuntimeLaunchResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAuditLogResponse {
    events: Vec<elastos_runtime::primitives::audit::AuditEvent>,
    total_in_memory: usize,
    current_epoch: u64,
}

#[derive(Serialize)]
struct HomeRuntimeSummary {
    running: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    pid: Option<u32>,
    #[serde(default)]
    running_capsules: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

impl Default for HomeRuntimeSummary {
    fn default() -> Self {
        Self {
            running: false,
            kind: None,
            version: Some(GATEWAY_VERSION.to_string()),
            api_url: None,
            pid: None,
            running_capsules: Vec::new(),
            note: None,
        }
    }
}

#[derive(Serialize)]
struct HomeSiteSummary {
    staged: bool,
    root_uri: String,
    path: String,
    #[serde(default)]
    active_release: Option<String>,
    #[serde(default)]
    active_channel: Option<String>,
    #[serde(default)]
    active_bundle_cid: Option<String>,
    release_count: usize,
}

impl Default for HomeSiteSummary {
    fn default() -> Self {
        Self {
            staged: false,
            root_uri: MY_WEBSITE_URI.to_string(),
            path: String::new(),
            active_release: None,
            active_channel: None,
            active_bundle_cid: None,
            release_count: 0,
        }
    }
}

#[derive(Default, Serialize)]
struct HomePendingRequestSummary {
    request_id: String,
    display_name: String,
    device_label: String,
    requested_at: u64,
}

#[derive(Default, Serialize)]
struct HomeActiveSessionSummary {
    display_name: String,
    device_label: String,
    approved_at: u64,
    last_seen_at: u64,
}

#[derive(Serialize)]
struct HomeRoomSummary {
    room_slug: String,
    title: String,
    member_count: usize,
    active_member_count: usize,
    pending_count: usize,
    active_session_count: usize,
    #[serde(default)]
    latest_request_name: Option<String>,
    #[serde(default)]
    latest_request_device: Option<String>,
    #[serde(default)]
    local_runtime_did: Option<String>,
    #[serde(default)]
    local_runtime_role: Option<String>,
    #[serde(default)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    ephemeral_hosted_guest_url: Option<String>,
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    pending_requests: Vec<HomePendingRequestSummary>,
    #[serde(default)]
    active_sessions: Vec<HomeActiveSessionSummary>,
}

impl Default for HomeRoomSummary {
    fn default() -> Self {
        Self {
            room_slug: crate::room_service::room_slug().to_string(),
            title: String::new(),
            member_count: 0,
            active_member_count: 0,
            pending_count: 0,
            active_session_count: 0,
            latest_request_name: None,
            latest_request_device: None,
            local_runtime_did: None,
            local_runtime_role: None,
            canonical_hosted_guest_url: None,
            ephemeral_hosted_guest_url: None,
            browser_access_allowed: true,
            browser_access_block_reason: None,
            pending_requests: Vec::new(),
            active_sessions: Vec::new(),
        }
    }
}

#[derive(Default, Serialize)]
struct HomeNotificationsSummary {
    unread_count: usize,
    attention_count: usize,
    #[serde(default)]
    entries: Vec<HomeNotificationEntrySummary>,
}

#[derive(Default, Serialize)]
struct HomeNotificationEntrySummary {
    id: String,
    source_app: String,
    kind: String,
    title: String,
    body: String,
    #[serde(default)]
    action_ref: Option<HomeNotificationActionSummary>,
    severity: String,
    read: bool,
    created_at: u64,
}

#[derive(Default, Serialize)]
struct HomeNotificationActionSummary {
    app: String,
    action_id: String,
}

#[derive(Default)]
struct HomeState {
    site: HomeSiteSummary,
    room: HomeRoomSummary,
    people: HomePeopleSummary,
    services: HomeServicesSummary,
    notifications: HomeNotificationsSummary,
}

/// One rendered size of a capsule's own app icon.
///
/// The route is a plain capsule asset route, so the shell fetches the icon
/// from the capsule that owns it instead of from a central shell icon table.
#[derive(Clone, Serialize)]
pub(in crate::api::gateway) struct CapsuleIconVariant {
    pub(in crate::api::gateway) size: u32,
    pub(in crate::api::gateway) route: String,
}

#[derive(Clone, Serialize)]
struct HomeTargetSummary {
    target: String,
    title: String,
    description: String,
    route: String,
    attach_kind: String,
    role: CapsuleRole,
    target_kind: HomeTargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_title: Option<String>,
    /// Empty when the capsule declares no icon; the shell then draws its own
    /// generic glyph rather than guessing at an asset route.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    icon: Vec<CapsuleIconVariant>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HomeLaunchRequest {
    target: String,
    #[serde(default)]
    query: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InboxActionRequest {
    action_id: String,
    #[serde(default)]
    step_up_token: Option<String>,
}

#[derive(Serialize)]
struct InboxActionResponse {
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PeopleContactRemoveRequest {
    contact_id: String,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
enum HomeTargetKind {
    App,
    Object,
}

#[derive(Serialize)]
struct HomeLaunchResponse {
    target: String,
    title: String,
    route: String,
    attach_kind: String,
    role: CapsuleRole,
    target_kind: HomeTargetKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    viewer_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    launch_detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capsule_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct ChatRoomSessionStartResponse {
    status: String,
    display_name: String,
    expires_at: u64,
    poll: GatewayRoomPollView,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomSummary {
    room_slug: String,
    pending_count: usize,
    active_session_count: usize,
    #[serde(default)]
    latest_request_name: Option<String>,
    #[serde(default)]
    latest_request_device: Option<String>,
    #[serde(default)]
    pending_requests: Vec<crate::room_service::PendingRequestView>,
    #[serde(default)]
    active_sessions: Vec<GatewayActiveSessionSummary>,
    #[serde(default)]
    room_control: GatewayRoomControlSummary,
    #[serde(default)]
    local_runtime_role: Option<crate::room_service::RoomRole>,
    #[serde(default)]
    canonical_hosted_guest_url: Option<String>,
    #[serde(default)]
    ephemeral_hosted_guest_url: Option<String>,
    browser_access_allowed: bool,
    #[serde(default)]
    browser_access_block_reason: Option<String>,
    #[serde(default)]
    transport: crate::room_service::RoomTransportView,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayActiveSessionSummary {
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

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomControlSummary {
    #[serde(default)]
    access_policy: crate::room_service::RoomAccessPolicyView,
    #[serde(default)]
    members: Vec<GatewayRoomMemberSummary>,
    #[serde(default)]
    pending_invites: Vec<GatewayRoomInviteSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomMemberSummary {
    role: String,
    added_at: u64,
    #[serde(default)]
    profile_card: Option<GatewayRoomProfileCardSummary>,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomProfileCardSummary {
    display_name: String,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomInviteSummary {
    invite_id: String,
    role: String,
    created_at: u64,
    expires_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayRoomPollView {
    room_slug: String,
    display_name: String,
    latest_seq: u64,
    #[serde(default)]
    participants: Vec<GatewayParticipantView>,
    #[serde(default)]
    objects: Vec<GatewayConversationObjectView>,
    #[serde(default)]
    transport: crate::room_service::RoomTransportView,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayConversationObjectView {
    seq: u64,
    sender: String,
    #[serde(default)]
    sender_profile_verified: Option<bool>,
    #[serde(default)]
    from_current_session: bool,
    kind: crate::room_service::ConversationObjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    link: Option<crate::room_service::LinkPreviewView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachment: Option<crate::room_service::AttachmentView>,
    created_at: u64,
}

#[derive(Debug, Clone, Serialize)]
struct GatewayParticipantView {
    display_name: String,
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

impl From<crate::room_service::RoomSummary> for GatewayRoomSummary {
    fn from(summary: crate::room_service::RoomSummary) -> Self {
        Self {
            room_slug: summary.room_slug,
            pending_count: summary.pending_count,
            active_session_count: summary.active_session_count,
            latest_request_name: summary.latest_request_name,
            latest_request_device: summary.latest_request_device,
            pending_requests: summary.pending_requests,
            active_sessions: summary
                .active_sessions
                .into_iter()
                .map(GatewayActiveSessionSummary::from)
                .collect(),
            room_control: GatewayRoomControlSummary::from(summary.room_control),
            local_runtime_role: summary.local_runtime_role,
            canonical_hosted_guest_url: summary.canonical_hosted_guest_url,
            ephemeral_hosted_guest_url: summary.ephemeral_hosted_guest_url,
            browser_access_allowed: summary.browser_access_allowed,
            browser_access_block_reason: summary.browser_access_block_reason,
            transport: summary.transport,
        }
    }
}

impl From<crate::room_service::ActiveSessionView> for GatewayActiveSessionSummary {
    fn from(session: crate::room_service::ActiveSessionView) -> Self {
        Self {
            session_id: session.session_id,
            display_name: session.display_name,
            device_label: session.device_label,
            approved_at: session.approved_at,
            expires_at: session.expires_at,
            last_seen_at: session.last_seen_at,
            capabilities: session.capabilities,
            member_bound: session.member_did.is_some(),
        }
    }
}

impl From<crate::room_service::RoomControlSummary> for GatewayRoomControlSummary {
    fn from(summary: crate::room_service::RoomControlSummary) -> Self {
        Self {
            access_policy: summary.access_policy,
            members: summary
                .members
                .into_iter()
                .map(GatewayRoomMemberSummary::from)
                .collect(),
            pending_invites: summary
                .pending_invites
                .into_iter()
                .map(GatewayRoomInviteSummary::from)
                .collect(),
        }
    }
}

impl From<crate::room_service::RoomMemberView> for GatewayRoomMemberSummary {
    fn from(view: crate::room_service::RoomMemberView) -> Self {
        Self {
            role: gateway_room_role_label(view.role),
            added_at: view.added_at,
            profile_card: view.profile_card.map(GatewayRoomProfileCardSummary::from),
        }
    }
}

impl From<crate::room_service::RoomProfileCardView> for GatewayRoomProfileCardSummary {
    fn from(view: crate::room_service::RoomProfileCardView) -> Self {
        Self {
            display_name: view.display_name,
        }
    }
}

impl From<crate::room_service::RoomInviteView> for GatewayRoomInviteSummary {
    fn from(view: crate::room_service::RoomInviteView) -> Self {
        Self {
            invite_id: view.invite_id,
            role: gateway_room_role_label(view.role),
            created_at: view.created_at,
            expires_at: view.expires_at,
        }
    }
}

impl From<crate::room_service::RoomPollView> for GatewayRoomPollView {
    fn from(view: crate::room_service::RoomPollView) -> Self {
        Self {
            room_slug: view.room_slug,
            display_name: view.display_name,
            latest_seq: view.latest_seq,
            participants: view
                .participants
                .into_iter()
                .map(GatewayParticipantView::from)
                .collect(),
            objects: view
                .objects
                .into_iter()
                .map(GatewayConversationObjectView::from)
                .collect(),
            transport: view.transport,
        }
    }
}

impl From<crate::room_service::ConversationObjectView> for GatewayConversationObjectView {
    fn from(view: crate::room_service::ConversationObjectView) -> Self {
        Self {
            seq: view.seq,
            sender: view.sender,
            sender_profile_verified: view.sender_profile_verified,
            from_current_session: view.from_current_session,
            kind: view.kind,
            body: view.body,
            emoji: view.emoji,
            link: view.link,
            attachment: view.attachment,
            created_at: view.created_at,
        }
    }
}

impl From<crate::room_service::ParticipantView> for GatewayParticipantView {
    fn from(view: crate::room_service::ParticipantView) -> Self {
        Self {
            display_name: view.display_name,
            profile_verified: view.profile_verified,
            device_label: view.device_label,
            last_seen_at: view.last_seen_at,
            role: view.role.map(gateway_room_role_label),
            local_session_count: view.local_session_count,
            is_current_session: view.is_current_session,
        }
    }
}

fn gateway_room_role_label(role: crate::room_service::RoomRole) -> String {
    match role {
        crate::room_service::RoomRole::Owner => "owner",
        crate::room_service::RoomRole::Admin => "admin",
        crate::room_service::RoomRole::Member => "member",
    }
    .to_string()
}
