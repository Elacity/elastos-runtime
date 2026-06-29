//! Public ElastOS edge for site roots, publisher objects, and CID content.
//!
//! Owns the browser-facing HTTP application routes and resolves them from
//! runtime-owned state (`MyWebSite`, `ElastOS/SystemServices/Publisher`, and
//! `ElastOS/SystemServices/Edge`) plus read-only CID content.

use std::collections::BTreeMap;
use std::convert::Infallible;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::documents::DocumentsClient;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::{
    header::{
        ACCEPT_RANGES, AUTHORIZATION, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE,
        CONTENT_TYPE, COOKIE, RANGE, SET_COOKIE,
    },
    HeaderMap, HeaderValue, StatusCode,
};
use axum::response::{
    sse::{Event as SseEvent, KeepAlive, Sse},
    Html, IntoResponse, Redirect, Response,
};
use axum::routing::{delete, get, post, put};
use axum::Json;
use axum::Router;
use base64::Engine as _;
use elastos_common::localhost::{
    edge_binding_path, edge_site_head_path, my_website_root_path, publisher_artifacts_path,
    publisher_install_script_path, publisher_release_head_path, publisher_release_manifest_path,
    publisher_site_releases_dir, rooted_localhost_fs_path, MY_WEBSITE_URI,
};
use elastos_common::{CapsuleRole, CapsuleType};
use elastos_identity::IdentityManager;
use elastos_runtime::auth::RuntimeAuditEventV1;
use elastos_runtime::provider::ProviderRegistry;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use url::form_urlencoded;

#[path = "gateway_browser.rs"]
mod gateway_browser;
#[path = "gateway_capsule_catalog.rs"]
mod gateway_capsule_catalog;
#[path = "gateway_home_runtime.rs"]
mod gateway_home_runtime;
#[path = "gateway_home_system.rs"]
mod gateway_home_system;
#[path = "gateway_home_token.rs"]
mod gateway_home_token;
#[path = "gateway_inbox.rs"]
mod gateway_inbox;
#[path = "gateway_marketplace.rs"]
mod gateway_marketplace;
#[path = "gateway_provider_proxy.rs"]
mod gateway_provider_proxy;
#[path = "gateway_room.rs"]
mod gateway_room;
#[path = "gateway_server.rs"]
mod gateway_server;
#[path = "gateway_site.rs"]
mod gateway_site;
#[path = "gateway_wallet.rs"]
mod gateway_wallet;
#[cfg(test)]
use gateway_browser::browser_runtime_stream_socket_path;
use gateway_capsule_catalog::*;
pub(crate) use gateway_home_runtime::is_wallet_connector_capsule_id;
use gateway_home_runtime::*;
pub(super) use gateway_home_runtime::{viewer_object_shell_description, viewer_object_shell_title};
use gateway_home_system::*;
pub(super) use gateway_home_token::{
    home_launch_token_header, home_session_clear_cookie_header,
    home_session_cookie_header_for_token, issue_home_launch_token_for_auth_grant,
    issue_home_launch_token_with_context, require_fresh_passkey_home_token,
    require_home_launch_token, require_home_launch_token_context,
    require_home_launch_token_for_any, require_home_launch_token_for_any_app_context,
    require_home_launch_token_for_any_context, require_home_token, require_home_token_context,
    HomeLaunchTokenContext,
};
#[cfg(test)]
use gateway_home_token::{
    issue_home_launch_token, local_home_launch_token_context, uuid_like_token,
};
use gateway_inbox::*;
use gateway_marketplace::*;
use gateway_provider_proxy::*;
use gateway_room::*;
pub(crate) use gateway_room::{
    cookie_value_from_headers, request_uses_tls, set_browser_session_cookie_header,
};
pub(crate) use gateway_server::advertised_gateway_urls;
pub use gateway_server::start_gateway_server;
use gateway_site::*;
pub(super) use gateway_site::{content_type, validate_file_path};
pub(crate) use gateway_wallet::ensure_wallet_connector_configured;
use gateway_wallet::*;

/// Maximum size for a single file fetched through the gateway (100 MB).
const MAX_GATEWAY_FILE_SIZE: usize = 100 * 1024 * 1024;
const LIBRARY_UPLOAD_CHUNK_MAX_BYTES: usize = 768 * 1024;
const GATEWAY_VERSION: &str = env!("ELASTOS_VERSION");
const MANAGED_WALLET_CHAIN_NAMESPACES: &[&str] = &[
    "eip155:20",
    "eip155:8453",
    "bip122:000000000019d6689c085ae165831e93",
];
const WALLET_PRICE_CACHE_TTL_SECS: u64 = 90;
const WALLET_PRICE_API_URL: &str = "https://api.coingecko.com/api/v3/simple/price";
const WALLET_PRICE_SOURCE_ENV: &str = "ELASTOS_WALLET_PRICE_SOURCE";
const WALLET_PRICE_HTTP_APPROVED_ENV: &str = "ELASTOS_WALLET_PRICE_HTTP_APPROVED";
const WALLET_PRICE_POLICY_SCHEMA: &str = "elastos.wallet.price-policy/v1";
const WALLET_PRICE_POLICY_ROOT: &str = "localhost://Local/Shared/System/Wallet";
const WALLET_PRICE_POLICY_FILE: &str = "price-policy.json";
const WALLET_PRICE_HTTP_REQUEST_ID: &str = "wallet-prices";
const WALLET_PRICE_HTTP_APPROVE_ACTION_ID: &str = "wallet-price-http-approve:coingecko";
const WALLET_PRICE_HTTP_DENY_ACTION_PREFIX: &str = "wallet-price-http-deny:";
const WALLET_PRICE_IDS: &[(&str, &str)] = &[
    ("BTC", "bitcoin"),
    ("ETH", "ethereum"),
    ("ELA", "elastos"),
    ("USDC", "usd-coin"),
    ("USDT", "tether"),
];
pub(crate) const ROOM_SESSION_COOKIE: &str = "room-session";
pub(crate) const BROWSER_SESSION_COOKIE: &str = "browser-session";
pub(crate) const HOME_SESSION_COOKIE: &str = "home-session";
const ROOM_SYNC_CONSUMER_ID: &str = "room-sync";
const HOME_LAUNCH_TOKEN_DOMAIN: &str = "elastos.home.launch.v1";
const HOME_LAUNCH_TOKEN_TTL_SECS: u64 = 12 * 60 * 60;
const HOME_BROWSER_STATE_SCHEMA: &str = "elastos.home.browser-state/v1";
const HOME_BROWSER_STATE_MAX_BYTES: usize = 64 * 1024;
const DOCUMENTS_CAPSULE_ID: &str = "documents";
const LIBRARY_CAPSULE_ID: &str = "library";
const MARKETPLACE_CAPSULE_ID: &str = "marketplace";
const INBOX_CAPSULE_ID: &str = "inbox";
pub(crate) const BROWSER_CAPSULE_ID: &str = "browser";
pub(crate) const SYSTEM_CAPSULE_ID: &str = "system";
const SYSTEM_ROUTE: &str = "/apps/system/";
const CHAT_ROOM_CAPSULE_ID: &str = "chat-room";
pub(crate) const WALLET_METAMASK_CAPSULE_ID: &str = "wallet-metamask";
pub(crate) const WALLET_CAPSULE_ID: &str = "wallet";
pub(crate) const WALLET_UNISAT_CAPSULE_ID: &str = "wallet-unisat";
pub(crate) const WALLET_WALLETCONNECT_CAPSULE_ID: &str = "wallet-walletconnect";
const WALLET_CONNECTOR_CAPSULE_IDS: &[&str] = &[
    WALLET_METAMASK_CAPSULE_ID,
    WALLET_UNISAT_CAPSULE_ID,
    WALLET_WALLETCONNECT_CAPSULE_ID,
];
pub(crate) const WALLET_LINK_CAPSULE_IDS: &[&str] = &[
    WALLET_METAMASK_CAPSULE_ID,
    WALLET_UNISAT_CAPSULE_ID,
    WALLET_WALLETCONNECT_CAPSULE_ID,
];
pub(crate) const HOME_CAPSULE_ID: &str = "home";
const HOME_ROUTE: &str = "/apps/home/";
pub(crate) const WALLETCONNECT_CONFIG_SCHEMA: &str = "elastos.walletconnect.connector/v1";
pub(crate) const WALLETCONNECT_CONFIG_PATH: &str =
    "ElastOS/SystemServices/WalletConnect/config.json";
pub(crate) const WALLETCONNECT_SDK_PATH: &str =
    "capsules/wallet-walletconnect/vendor/reown-appkit.js";
pub(crate) const WALLETCONNECT_SDK_PACKAGE: &str = "@reown/appkit";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletConnectConnectorConfig {
    schema: String,
    project_id: String,
    sdk_package: String,
    sdk_version: String,
    sdk_sha256: String,
}

static WALLET_PRICE_CACHE: OnceLock<tokio::sync::Mutex<Option<WalletPriceCache>>> = OnceLock::new();

#[derive(Debug, Clone)]
struct WalletPriceCache {
    as_of: u64,
    prices: BTreeMap<String, WalletPriceQuote>,
}

#[derive(Debug, Clone, Serialize)]
struct WalletPriceQuote {
    usd: f64,
    #[serde(rename = "change24h")]
    change_24h: f64,
}

#[derive(Debug, Serialize)]
struct WalletPricesResponse {
    #[serde(rename = "asOf")]
    as_of: u64,
    stale: bool,
    unavailable: bool,
    prices: BTreeMap<String, WalletPriceQuote>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WalletPricePolicy {
    schema: String,
    source: String,
    external_http_approved: bool,
    approved_by_principal_id: String,
    approved_at: u64,
}

#[derive(Clone)]
pub struct GatewayState {
    pub provider_registry: Option<Arc<ProviderRegistry>>,
    pub identity_manager: Arc<OnceLock<Arc<tokio::sync::Mutex<IdentityManager>>>>,
    pub cache_dir: PathBuf,
    /// Runtime data directory backing rooted Publisher/Edge/MyWebSite state.
    pub data_dir: PathBuf,
    /// Tamper-evident audit sink (GAP-8), lazily file-backed under `data_dir/audit/`. Mirrors the
    /// `identity_manager` lazy pattern: an EXPLICIT field (no hidden global), initialized on first
    /// use so the ~25 constructors stay `Arc::new(OnceLock::new())`. FOLLOW-ON: unify this with the
    /// runtime/infra audit sink so the gateway and runtime share ONE custody log.
    pub audit_log: Arc<OnceLock<Arc<elastos_runtime::primitives::audit::AuditLog>>>,
    /// The SAME shared act-spend meter the carrier paths use (`infra.spend_policy`), threaded in at
    /// serve time so a capsule's budget is unified across planes — the gateway debits the same meter
    /// as serve/microVM/WASM, not a separate one. `None` ⇒ unmetered (tests + control plane).
    pub spend_policy: Option<crate::carrier_bridge::SpendPolicy>,
}

impl GatewayState {
    /// The tamper-evident audit log, lazily opened (hash-chained + ed25519-signed) under
    /// `data_dir/audit/gateway-audit.log`. FAIL-CLOSED (AUD-2): if the signed, durable file
    /// log cannot be opened this returns `Err` rather than silently swapping in an unsigned,
    /// fail-open memory log. A custody caller must then refuse to serve — content whose open
    /// is recorded only to a volatile, non-tamper-evident memory log must never be served.
    pub(crate) fn audit_log(
        &self,
    ) -> Result<Arc<elastos_runtime::primitives::audit::AuditLog>, String> {
        use elastos_runtime::primitives::audit::AuditLog;
        if let Some(existing) = self.audit_log.get() {
            return Ok(existing.clone());
        }
        let path = self.data_dir.join("audit").join("gateway-audit.log");
        let log = Arc::new(
            AuditLog::with_file(&path)
                .map_err(|e| format!("gateway audit log unavailable at {path:?}: {e}"))?,
        );
        let _ = self.audit_log.set(log.clone());
        // Another thread may have won the race; return whatever is now canonical.
        Ok(self.audit_log.get().cloned().unwrap_or(log))
    }

    pub(crate) fn identity_manager(
        &self,
    ) -> anyhow::Result<Arc<tokio::sync::Mutex<IdentityManager>>> {
        if let Some(manager) = self.identity_manager.get() {
            return Ok(manager.clone());
        }

        let manager = Arc::new(tokio::sync::Mutex::new(IdentityManager::new(
            self.data_dir.clone(),
        )?));
        if self.identity_manager.set(manager.clone()).is_ok() {
            return Ok(manager);
        }
        self.identity_manager
            .get()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("passkey identity manager unavailable"))
    }
}

pub fn gateway_router(state: GatewayState) -> Router {
    Router::new()
        .route("/", get(serve_public_root))
        .route("/healthz", get(healthz))
        .route(
            "/api/auth/evm/challenge",
            post(super::auth_gateway::evm_challenge),
        )
        .route(
            "/api/auth/evm/verify",
            post(super::auth_gateway::evm_verify),
        )
        .route(
            "/api/auth/btc/challenge",
            post(super::auth_gateway::btc_challenge),
        )
        .route(
            "/api/auth/btc/verify",
            post(super::auth_gateway::btc_verify),
        )
        .route(
            "/api/auth/sessions/:session_id/revoke",
            post(super::auth_gateway::revoke_session),
        )
        .route(
            "/api/auth/sessions/sign-out",
            post(super::auth_gateway::sign_out_session),
        )
        .route(
            "/api/auth/sessions/refresh",
            post(super::auth_gateway::refresh_session),
        )
        .route(
            "/api/auth/passkey/status",
            get(super::auth_gateway::passkey_status),
        )
        .route("/api/auth/passkeys", get(super::auth_gateway::passkey_list))
        .route(
            "/api/auth/recovery/status",
            get(super::auth_gateway::recovery_status),
        )
        .route(
            "/api/auth/recovery/create",
            post(super::auth_gateway::recovery_kit_create),
        )
        .route(
            "/api/auth/recovery/export",
            post(super::auth_gateway::recovery_kit_export),
        )
        .route(
            "/api/auth/recovery/import",
            post(super::auth_gateway::recovery_kit_import),
        )
        .route(
            "/api/auth/recovery/full-export",
            post(super::auth_gateway::full_recovery_bundle_export),
        )
        .route(
            "/api/auth/recovery/full-import",
            post(super::auth_gateway::full_recovery_bundle_import),
        )
        .route(
            "/api/auth/passkeys/:proof_binding_id/revoke",
            post(super::auth_gateway::passkey_revoke),
        )
        .route(
            "/api/auth/passkeys/:proof_binding_id/promote-admin",
            post(super::auth_gateway::passkey_promote_admin),
        )
        .route(
            "/api/auth/passkeys/:proof_binding_id/demote-guest",
            post(super::auth_gateway::passkey_demote_guest),
        )
        .route(
            "/api/auth/passkey/register/begin",
            post(super::auth_gateway::passkey_register_begin),
        )
        .route(
            "/api/auth/passkey/register/complete",
            post(super::auth_gateway::passkey_register_complete),
        )
        .route(
            "/api/auth/passkey/authenticate/begin",
            post(super::auth_gateway::passkey_authenticate_begin),
        )
        .route(
            "/api/auth/passkey/authenticate/complete",
            post(super::auth_gateway::passkey_authenticate_complete),
        )
        .route(
            "/api/browser/session/request",
            post(super::browser_sessions::browser_session_request),
        )
        .route(
            "/api/browser/session/request/:request_id",
            get(super::browser_sessions::browser_session_request_status),
        )
        .route(
            "/api/provider/object/events/stream",
            get(gateway_library_events_stream),
        )
        .route(
            "/api/provider/object/download/raw",
            get(gateway_library_download),
        )
        .route("/api/provider/object/cover", get(gateway_library_cover))
        .route(
            "/api/provider/object/upload",
            put(gateway_library_upload).layer(DefaultBodyLimit::max(MAX_GATEWAY_FILE_SIZE)),
        )
        .route(
            "/api/provider/object/upload/start",
            post(gateway_library_upload_start),
        )
        .route(
            "/api/provider/object/upload/:upload_id/chunk",
            put(gateway_library_upload_chunk)
                .layer(DefaultBodyLimit::max(LIBRARY_UPLOAD_CHUNK_MAX_BYTES)),
        )
        .route(
            "/api/provider/object/upload/:upload_id/finish",
            post(gateway_library_upload_finish),
        )
        .route(
            "/api/provider/object/upload/:upload_id",
            delete(gateway_library_upload_cancel),
        )
        .route("/api/provider/:scheme/:op", post(gateway_provider_proxy))
        .route("/release.json", get(serve_release_manifest))
        .route("/release-head.json", get(serve_release_head))
        .route("/install.sh", get(serve_install_script))
        .route(
            "/.well-known/elastos/site-head.json",
            get(serve_site_head_document),
        )
        .route("/artifacts/*path", get(serve_artifact_file))
        .route("/api/apps/system/summary", get(system_summary))
        .route(
            "/api/apps/system/identity/handle",
            post(system_handle_update),
        )
        .route(
            "/api/apps/system/appearance/background-image",
            post(system_background_image_update)
                .delete(system_background_image_reset)
                .layer(DefaultBodyLimit::max(
                    HOME_BACKGROUND_IMAGE_TRANSPORT_MAX_BYTES,
                )),
        )
        .route(
            "/api/apps/system/appearance/background-overlay",
            post(system_background_overlay_update),
        )
        .route(
            "/api/apps/system/access/guest-registration",
            post(system_guest_registration_update),
        )
        .route(
            "/api/apps/system/wallet/approvals",
            get(system_wallet_approvals),
        )
        .route(
            "/api/apps/system/wallet/managed",
            post(system_wallet_managed_create),
        )
        .route(
            "/api/apps/system/wallet/default",
            post(system_wallet_default_update),
        )
        .route(
            "/api/apps/system/wallet/approvals/:request_id/reject",
            post(system_wallet_approval_reject),
        )
        .route(
            "/api/apps/system/wallet/approvals/:request_id/approve",
            post(system_wallet_approval_approve),
        )
        .route("/api/apps/wallet/wallet/summary", get(wallet_app_summary))
        .route("/api/wallet/prices", get(wallet_prices))
        .route("/api/wallet/qr", post(wallet_receive_qr))
        .route(
            "/api/apps/wallet/wallet/managed",
            post(wallet_app_managed_create),
        )
        .route(
            "/api/apps/wallet/wallet/default",
            post(wallet_app_default_update),
        )
        .route(
            "/api/apps/wallet/wallet/accounts/:account_id",
            delete(wallet_app_account_delete).put(wallet_app_account_rename),
        )
        .route(
            "/api/apps/wallet/wallet/accounts/:account_id/recovery-key",
            post(wallet_app_account_recovery_key),
        )
        .route(
            "/api/apps/wallet/wallet/accounts/import-recovery-key",
            post(wallet_app_account_import_recovery_key),
        )
        .route(
            "/api/apps/wallet/wallet/approvals/:request_id/reject",
            post(wallet_app_approval_reject),
        )
        .route(
            "/api/apps/wallet/wallet/approvals/:request_id/approve",
            post(wallet_app_external_approval_approve),
        )
        .route(
            "/api/apps/wallet/wallet/approvals/:request_id/complete",
            post(wallet_app_external_approval_complete),
        )
        .route(
            "/api/apps/wallet/wallet/managed-approvals/:request_id/approve",
            post(wallet_app_managed_approval_approve),
        )
        .route(
            "/api/apps/wallet/wallet/send",
            post(wallet_app_send_transaction),
        )
        .route(
            "/api/apps/browser/summary",
            get(gateway_browser::browser_app_summary),
        )
        .route(
            "/api/apps/browser/open",
            post(gateway_browser::browser_app_open),
        )
        .route(
            "/api/apps/browser/pages/:page_id/status",
            get(gateway_browser::browser_app_page_status),
        )
        .route(
            "/api/apps/browser/pages/:page_id/heartbeat",
            post(gateway_browser::browser_app_page_heartbeat),
        )
        .route(
            "/api/apps/browser/pages/:page_id/screenshot",
            get(gateway_browser::browser_app_page_screenshot),
        )
        .route(
            "/api/apps/browser/pages/:page_id/frame",
            get(gateway_browser::browser_app_page_frame),
        )
        .route(
            "/api/apps/browser/pages/:page_id/input",
            post(gateway_browser::browser_app_page_input),
        )
        .route(
            "/api/apps/browser/pages/:page_id/close",
            post(gateway_browser::browser_app_page_close),
        )
        .route(
            "/api/apps/browser/pages/:page_id/webrtc",
            post(gateway_browser::browser_app_page_webrtc),
        )
        .route(
            "/api/apps/browser/wallet/request-signature",
            post(gateway_browser::browser_app_wallet_request_signature)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/browser/wallet/bridge",
            get(gateway_browser::browser_app_wallet_bridge)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/browser/wallet/request-transaction",
            post(gateway_browser::browser_app_wallet_request_transaction)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/browser/wallet/read",
            post(gateway_browser::browser_app_wallet_read)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/browser/wallet/broadcast-transaction",
            post(gateway_browser::browser_app_wallet_broadcast_transaction)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/browser/wallet/approvals/:request_id",
            get(gateway_browser::browser_app_wallet_approval_status)
                .options(gateway_browser::browser_app_wallet_cors_preflight),
        )
        .route(
            "/api/apps/:wallet_connector/wallet/approvals",
            get(wallet_connector_approvals),
        )
        .route(
            "/api/apps/:wallet_connector/wallet/config",
            get(wallet_connector_config),
        )
        .route(
            "/api/apps/:wallet_connector/wallet/accounts",
            get(wallet_connector_accounts),
        )
        .route(
            "/api/apps/:wallet_connector/wallet/approvals/:request_id/approve",
            post(wallet_connector_approval_approve),
        )
        .route(
            "/api/apps/:wallet_connector/wallet/approvals/:request_id/complete",
            post(wallet_connector_approval_complete),
        )
        .route("/api/apps/home/summary", get(home_summary))
        .route("/api/apps/home/events", get(home_events))
        .route("/api/apps/home/events/stream", get(home_events_stream))
        .route(
            "/api/apps/home/state",
            get(home_browser_state_get).post(home_browser_state_update),
        )
        .route(
            "/api/apps/home/appearance/background-image",
            get(home_background_image),
        )
        .route("/api/apps/home/runtime/ensure", post(home_runtime_ensure))
        .route("/api/apps/home/launch", post(home_launch))
        .route("/api/capsules/catalog", get(capsule_catalog))
        .route("/api/capsules/interfaces", get(capsule_interfaces))
        .route(
            "/api/capsules/interfaces/invoke",
            post(capsule_interface_invoke),
        )
        .route("/api/apps/marketplace/catalog", get(marketplace_catalog))
        .route("/api/apps/inbox/summary", get(inbox_summary))
        .route("/api/apps/inbox/actions", post(inbox_action))
        .route("/api/apps/chat-room/summary", get(chat_room_summary))
        .route(
            "/api/apps/chat-room/requests/:request_id/approve",
            post(chat_room_request_approve),
        )
        .route(
            "/api/apps/chat-room/requests/:request_id/deny",
            post(chat_room_request_deny),
        )
        .route(
            "/api/apps/chat-room/guests/:session_id/kick",
            post(chat_room_guest_kick),
        )
        .route(
            "/api/apps/chat-room/access-policy",
            post(chat_room_access_policy_update),
        )
        .route(
            "/api/apps/chat-room/members/invite",
            post(chat_room_member_invite),
        )
        .route(
            "/api/apps/chat-room/members/remove",
            post(chat_room_member_remove),
        )
        .route(
            "/api/apps/chat-room/invites/revoke",
            post(chat_room_invite_revoke),
        )
        .route(
            "/api/apps/chat-room/session/start",
            post(chat_room_session_start),
        )
        .route(
            "/api/apps/chat-room/session/leave",
            post(room_service_session_leave),
        )
        .route("/api/apps/chat-room/poll", post(room_service_poll))
        .route(
            "/api/apps/chat-room/objects/send",
            post(room_service_objects_send),
        )
        .route(
            "/api/apps/chat-room/upload/start",
            post(room_service_upload_start),
        )
        .route(
            "/api/apps/chat-room/upload/:upload_id/chunk",
            post(room_service_upload_chunk),
        )
        .route(
            "/api/apps/chat-room/upload/:upload_id/finish",
            post(room_service_upload_finish),
        )
        .route(
            "/api/apps/chat-room/attachments/:attachment_id",
            get(room_service_attachment_get),
        )
        .route(
            "/api/viewers/:viewer/library",
            get(super::viewer_gateway::viewer_library_summary),
        )
        .route(
            "/api/viewers/:viewer/content/:capsule",
            get(super::viewer_gateway::viewer_content),
        )
        .route(
            "/api/viewers/:viewer/library-object",
            get(super::viewer_gateway::viewer_library_object_get)
                .post(super::viewer_gateway::viewer_library_object_post)
                .put(super::viewer_gateway::viewer_library_object_put),
        )
        .route(
            "/api/viewers/:viewer/library-roots",
            get(super::viewer_gateway::viewer_library_roots_get),
        )
        .route(
            "/api/viewers/:viewer/storage/:capsule/:scope/:name",
            get(super::viewer_gateway::viewer_storage_get)
                .put(super::viewer_gateway::viewer_storage_put),
        )
        .route(
            "/api/viewers/open",
            axum::routing::post(super::viewer_open::open_owned_in_viewer),
        )
        .route(
            "/api/viewers/prepare-grant",
            axum::routing::post(super::viewer_open::prepare_owned_grant),
        )
        .route(
            "/api/market/buy",
            axum::routing::post(super::viewer_open::buy_owned_access),
        )
        .route(
            "/api/create/mint",
            axum::routing::post(super::viewer_open::mint_create_asset),
        )
        .route(
            "/api/apps/creator/status",
            get(super::creator::creator_status),
        )
        .route(
            "/api/apps/creator/prepare-mint",
            // The mint payload carries the asset bytes (base64) inline, so it far exceeds axum's
            // 2 MB default body cap. Allow the same ceiling as a Library upload (100 MB) — base64
            // inflation (~33%) means this comfortably covers an image and a modest video.
            axum::routing::post(super::creator::creator_prepare_mint)
                .layer(DefaultBodyLimit::max(MAX_GATEWAY_FILE_SIZE)),
        )
        .route(
            "/api/apps/creator/prepare-progress/:job_id",
            get(super::creator::creator_prepare_progress),
        )
        .route(
            "/api/apps/creator/wallet",
            get(super::creator::creator_wallet),
        )
        .route(
            "/api/apps/creator/channels",
            get(super::creator::creator_list_channels),
        )
        .route(
            "/api/apps/creator/create-channel",
            axum::routing::post(super::creator::creator_create_channel),
        )
        .route(
            "/api/apps/creator/prepare-trade-approval",
            axum::routing::post(super::creator::creator_prepare_trade_approval),
        )
        .route(
            "/api/apps/creator/mint-status",
            axum::routing::post(super::creator::creator_mint_status),
        )
        .route(
            "/api/viewers/elacity-player/media/open",
            axum::routing::post(super::media_authority::open_demo_media),
        )
        .route(
            "/api/viewers/:viewer/media/:session",
            get(super::viewer_media::viewer_media_manifest),
        )
        .route(
            "/api/viewers/:viewer/media/:session/init",
            get(super::viewer_media::viewer_media_init),
        )
        .route(
            "/api/viewers/:viewer/media/:session/cover",
            get(super::viewer_media::viewer_media_cover),
        )
        .route(
            "/api/viewers/:viewer/media/:session/segment/:index",
            get(super::viewer_media::viewer_media_segment),
        )
        .route(
            "/api/viewers/:viewer/media/:session/track/:track/init",
            get(super::viewer_media::viewer_media_track_init),
        )
        .route(
            "/api/viewers/:viewer/media/:session/track/:track/segment/:index",
            get(super::viewer_media::viewer_media_track_segment),
        )
        .route(
            "/api/viewers/ddrm-viewer/object/open",
            axum::routing::post(super::object_authority::open_owned_object),
        )
        .route(
            "/api/viewers/:viewer/object/:session",
            get(super::viewer_object::viewer_object_manifest),
        )
        .route(
            "/api/viewers/:viewer/object/:session/bytes",
            get(super::viewer_object::viewer_object_bytes),
        )
        .route(
            "/api/viewers/:viewer/object/:session/page",
            get(super::viewer_object::viewer_object_page),
        )
        .route(
            "/apps/:app",
            get(super::browser_capsules::serve_browser_app_root),
        )
        .route(
            "/apps/:app/",
            get(super::browser_capsules::serve_browser_app_index),
        )
        .route(
            "/apps/:app/*path",
            get(super::browser_capsules::serve_browser_app_asset),
        )
        .route("/s/:cid", get(redirect_cid_root))
        .route("/s/:cid/", get(serve_cid_root))
        .route("/s/:cid/*path", get(serve_cid_file))
        .route("/content/:cid", get(serve_ipfs_cid_root))
        // IPFS-compatible paths so install.sh can use this gateway like ipfs.io
        .route("/ipfs/:cid", get(serve_ipfs_cid_root))
        .route("/ipfs/:cid/", get(serve_cid_root))
        .route("/ipfs/:cid/*path", get(serve_cid_file))
        .route("/*path", get(serve_public_site_path))
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn landing_page() -> Html<String> {
    Html(format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>ElastOS Gateway</title>
  <style>
    :root {{
      --bg: #0c1217;
      --panel: #111a21;
      --text: #e9f0f4;
      --muted: #9bb1bf;
      --accent: #29b6f6;
      --ok: #54d66d;
      --border: #203241;
    }}
    body {{
      margin: 0;
      font-family: "Segoe UI", "SF Pro Text", system-ui, sans-serif;
      background: radial-gradient(circle at 20% 0%, #122130 0%, var(--bg) 45%);
      color: var(--text);
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 1.25rem;
    }}
    main {{
      width: min(820px, 100%);
      background: linear-gradient(180deg, #111a21 0%, #0f171f 100%);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 1.25rem;
      box-shadow: 0 20px 40px rgba(0, 0, 0, 0.35);
    }}
    h1 {{
      margin: 0 0 0.25rem;
      font-size: 1.5rem;
    }}
    .muted {{
      color: var(--muted);
      margin: 0.125rem 0 0.75rem;
    }}
    .version {{
      font-size: 0.875rem;
      color: var(--ok);
      margin-bottom: 1rem;
    }}
    form {{
      display: flex;
      gap: 0.5rem;
      margin: 1rem 0 0.75rem;
      flex-wrap: wrap;
    }}
    input[type="text"] {{
      flex: 1 1 420px;
      min-width: 220px;
      background: #0d141b;
      color: var(--text);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 0.7rem 0.8rem;
      font-size: 0.95rem;
    }}
    button {{
      background: var(--accent);
      color: #05131d;
      border: 0;
      border-radius: 10px;
      padding: 0.7rem 1rem;
      font-weight: 700;
      cursor: pointer;
    }}
    code {{
      background: #0c141b;
      border: 1px solid var(--border);
      border-radius: 6px;
      padding: 0.1rem 0.35rem;
    }}
    ul {{
      margin-top: 0.5rem;
      color: var(--muted);
    }}
    a {{
      color: var(--accent);
    }}
  </style>
</head>
<body>
  <main>
    <h1>ElastOS Gateway</h1>
    <p class="muted">Public ElastOS edge for MyWebSite, publisher objects, and read-only CID content.</p>
    <p class="version">Version {}</p>

    <form id="cid-form">
      <input id="cid-input" type="text" placeholder="Paste CID, elastos://CID, or gateway URL" autocomplete="off" />
      <button type="submit">Open</button>
    </form>

    <p class="muted">URL format: <code>/s/&lt;cid&gt;/</code></p>
    <ul>
      <li>Health check: <a href="/healthz">/healthz</a></li>
      <li>Site root: <code>/</code> from <code>localhost://MyWebSite</code> or a bound Edge target.</li>
      <li>Publisher objects: <code>/release-head.json</code>, <code>/release.json</code>, <code>/install.sh</code>, <code>/artifacts/...</code></li>
      <li>Content example: <code>/s/bafy.../</code></li>
    </ul>
  </main>

  <script>
    (function () {{
      function extractCid(input) {{
        var s = (input || "").trim().replace(/\/+$/, "");
        if (!s) return "";
        if (s.startsWith("elastos://")) return s.slice("elastos://".length).split("/")[0];
        var m1 = s.match(/\/ipfs\/([^\/?#]+)/);
        if (m1 && m1[1]) return m1[1];
        var m2 = s.match(/^https?:\/\/([^./]+)\.ipfs\./i);
        if (m2 && m2[1]) return m2[1];
        return s;
      }}

      var form = document.getElementById("cid-form");
      var input = document.getElementById("cid-input");
      if (!form || !input) return;
      form.addEventListener("submit", function (e) {{
        e.preventDefault();
        var cid = extractCid(input.value);
        if (!cid) return;
        window.location.href = "/s/" + encodeURIComponent(cid) + "/";
      }});
    }})();
  </script>
</body>
</html>"#,
        GATEWAY_VERSION
    ))
}

include!("gateway_models.rs");

fn load_existing_gateway_runtime_did(data_dir: &std::path::Path) -> Option<String> {
    // The gateway DID is deterministically derived from the on-disk device key
    // (SHA-256 over a fixed label || device_key) and is therefore boot-stable.
    // Per-request token verification calls this on every protected-asset fetch, so
    // re-reading device.key + re-deriving the ed25519 identity each time is pure
    // waste. Memoize the resolved DID instead.
    //
    // Containment: only the POSITIVE result is cached. A missing identity keeps
    // being re-checked (so a device key provisioned after gateway start is still
    // picked up), and the cached value can only ever be the real derived DID — it
    // can never widen authority. The signature/expiry/session checks that actually
    // authorize a request stay per-request in the callers; this only memoizes WHO
    // the trusted signer is, not WHETHER a given token is valid.
    static DID_CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, String>>,
    > = std::sync::OnceLock::new();
    let cache = DID_CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));

    if let Some(did) = cache.lock().ok().and_then(|m| m.get(data_dir).cloned()) {
        return Some(did);
    }

    let device_key = data_dir.join("identity").join("device.key");
    if !device_key.exists() {
        return None;
    }
    let did = elastos_identity::load_or_create_did(data_dir)
        .ok()
        .map(|(_signing_key, did)| did)
        .filter(|did| !did.trim().is_empty())?;
    if let Ok(mut m) = cache.lock() {
        m.insert(data_dir.to_path_buf(), did.clone());
    }
    Some(did)
}

#[derive(Debug, Clone, Deserialize)]
struct GatewayRuntimeCoords {
    api_url: String,
    attach_secret: String,
}

#[derive(Debug, Deserialize)]
struct GatewayAttachResponse {
    token: String,
}

fn load_runtime_coords(data_dir: &std::path::Path) -> Option<GatewayRuntimeCoords> {
    let path = data_dir.join("runtime-coords.json");
    let bytes = std::fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn gateway_internal_error(err: anyhow::Error) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": err.to_string() })),
    )
}

fn gateway_provider_error_response(scheme: &str, err: anyhow::Error) -> Response {
    let text = err.to_string();
    let unavailable = format!("{scheme} provider unavailable");
    let forbidden = text.contains("home launch token")
        || text.contains("not allowlisted")
        || text.contains("No Browser Exit backend allows host")
        || text.contains("policy blocked")
        || text.contains("private host blocked")
        || text.contains("private IP blocked")
        || text.contains("private network");
    let status = if forbidden {
        StatusCode::FORBIDDEN
    } else if text.contains(&unavailable) {
        StatusCode::SERVICE_UNAVAILABLE
    } else if text.contains("not found") {
        StatusCode::NOT_FOUND
    } else if text.contains("invalid")
        || text.contains("empty")
        || text.contains("must not")
        || text.contains("missing")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn inbox_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") || text.contains("fresh passkey") {
        StatusCode::FORBIDDEN
    } else if text.contains("unknown inbox action") || text.contains("Open Wallet") {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn system_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token")
        || text.contains("admin passkey required")
        || text.contains("proof-bound passkey session required")
        || text.contains("fresh passkey")
    {
        StatusCode::FORBIDDEN
    } else if text.contains("nickname must")
        || text.contains("missing")
        || text.contains("background image")
        || text.contains("wallet connector")
        || text.contains("WalletConnect connector")
        || text.contains("approval method")
        || text.contains("built-in wallet request")
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

fn home_error_response(err: anyhow::Error) -> Response {
    let text = err.to_string();
    let status = if text.contains("home launch token") || text.contains("gateway identity") {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    };
    (status, text).into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[path = "gateway_browser_tests.rs"]
mod gateway_browser_tests;
#[cfg(test)]
#[path = "gateway_tests/mod.rs"]
mod gateway_tests;

#[cfg(test)]
mod aud2_audit_failclosed_tests {
    use super::*;

    #[test]
    fn audit_log_fails_closed_when_signed_log_cannot_open() {
        // AUD-2: when the signed file log cannot be opened, audit_log() must return Err
        // (so the custody caller refuses to serve), NOT silently fall back to an unsigned,
        // fail-open memory log. Force the failure by placing a FILE where the audit/ dir
        // would go, so AuditLog::with_file cannot create+open data_dir/audit/...log.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("audit"), b"not a directory").unwrap();
        let state = GatewayState {
            provider_registry: None,
            identity_manager: Arc::new(std::sync::OnceLock::new()),
            cache_dir: dir.path().to_path_buf(),
            data_dir: dir.path().to_path_buf(),
            audit_log: Arc::new(std::sync::OnceLock::new()),
            spend_policy: None,
        };
        assert!(
            state.audit_log().is_err(),
            "audit_log must fail closed when the signed file log cannot open, never fall back to a memory log"
        );
    }
}
