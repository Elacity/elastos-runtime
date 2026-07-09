//! API request handlers
//!
//! This module contains the handler functions for the HTTP API endpoints.

pub mod attach;
pub mod capability;
pub mod docs;
pub mod identity;
pub mod namespace;
pub mod provider;
pub mod storage;

pub use capability::{
    deny_request, dispatch_agent_intent, dispatch_standing_intent, get_audit_event_types,
    get_audit_log, get_spend_budget, grant_request, issue_standing_grant, list_capabilities,
    list_pending, list_pending_payments, list_standing_grants, mandate_receipt,
    preview_standing_grant, reconcile_payment, request_capability, request_status,
    revoke_all_capabilities, revoke_capability, revoke_standing_grant, session_info,
    set_spend_budget, validate_and_consume, CapabilityState,
};

pub use namespace::{
    cache_status, delete_path, list_path, namespace_status, prefetch_content, read_content,
    resolve_path, write_content, NamespaceState,
};

pub use storage::{
    delete_path as storage_delete, handle_get as storage_get, handle_get_root as storage_get_root,
    handle_post as storage_post, stat_path as storage_stat, write_file as storage_write,
};

pub mod orchestrator;
pub mod supervisor_api;

#[cfg(debug_assertions)]
pub mod test_helpers;
