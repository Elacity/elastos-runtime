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
    device_did: Option<String>,
    handle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_card: Option<HomeProfileCardSummary>,
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
    discovery: HomePeopleDiscoverySummary,
}

impl Default for HomePeopleSummary {
    fn default() -> Self {
        Self {
            schema: "elastos.people.contacts/v1".to_string(),
            contact_count: 0,
            contacts: Vec::new(),
            service_offer_count: 0,
            service_offers: Vec::new(),
            discovery: HomePeopleDiscoverySummary::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct HomePeopleDiscoverySummary {
    schema: String,
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    remaining_seconds: Option<u64>,
    visibility: String,
    status: String,
    status_message: String,
    topic: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_peer_id: Option<String>,
    discovered_count: usize,
    #[serde(default)]
    discovered_peers: Vec<HomePeopleDiscoveryPeerSummary>,
    request_count: usize,
    #[serde(default)]
    requests: Vec<HomePeopleDiscoveryRequestSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    changed: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refresh_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    next_refresh_after_ms: Option<u64>,
}

impl Default for HomePeopleDiscoverySummary {
    fn default() -> Self {
        Self {
            schema: "elastos.people.discovery/v1".to_string(),
            enabled: false,
            expires_at: None,
            remaining_seconds: None,
            visibility: "off".to_string(),
            status: "off".to_string(),
            status_message: "Discovery is off.".to_string(),
            topic: "__elastos_internal/people-discovery-v1".to_string(),
            local_peer_id: None,
            discovered_count: 0,
            discovered_peers: Vec::new(),
            request_count: 0,
            requests: Vec::new(),
            changed: None,
            refresh_fingerprint: None,
            next_refresh_after_ms: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleDiscoveryPeerSummary {
    peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    last_seen_at: u64,
    status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct HomePeopleDiscoveryRequestSummary {
    request_id: String,
    peer_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    did: Option<String>,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    created_at: u64,
    status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    invite_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct HomePeopleContactSummary {
    contact_id: String,
    #[serde(skip)]
    added_at: u64,
    display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    handle: Option<String>,
    relationship: String,
    route: String,
    can_message: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    device_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    profile_card: Option<HomeProfileCardSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen_at: Option<u64>,
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
struct SystemHandleUpdateRequest {
    handle: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemBackgroundOverlayRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default = "home_background_overlay_opacity_default")]
    opacity: f64,
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

#[derive(Debug, Clone, Default, Serialize)]
struct HomeAppearanceSummary {
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

// `pub(in crate::api)`, not private: `viewer_open::prepare_owned_grant` (a sibling of `gateway`)
// calls `system_wallet_accounts_summary`/`default_evm_connector_id`, both of which take/return
// this type in their public interface — it must be at least as visible as those functions for
// callers outside `gateway` to use them. The nested `SystemWalletAccountSummary` /
// `SystemWalletDefaultSummary` stay private; only this outer type is named across the boundary.
#[derive(Debug, Clone, Default, Serialize)]
pub(in crate::api) struct SystemWalletAccountsSummary {
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
    active_participants: Vec<crate::room_service::ParticipantView>,
    #[serde(default)]
    pending_requests: Vec<crate::room_service::PendingRequestView>,
    #[serde(default)]
    active_sessions: Vec<GatewayActiveSessionSummary>,
    #[serde(default)]
    room_control: crate::room_service::RoomControlSummary,
    #[serde(default)]
    local_runtime_did: Option<String>,
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
    member_did: Option<String>,
}

impl From<crate::room_service::RoomSummary> for GatewayRoomSummary {
    fn from(summary: crate::room_service::RoomSummary) -> Self {
        Self {
            room_slug: summary.room_slug,
            pending_count: summary.pending_count,
            active_session_count: summary.active_session_count,
            latest_request_name: summary.latest_request_name,
            latest_request_device: summary.latest_request_device,
            active_participants: summary.active_participants,
            pending_requests: summary.pending_requests,
            active_sessions: summary
                .active_sessions
                .into_iter()
                .map(GatewayActiveSessionSummary::from)
                .collect(),
            room_control: summary.room_control,
            local_runtime_did: summary.local_runtime_did,
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
            member_did: session.member_did,
        }
    }
}
