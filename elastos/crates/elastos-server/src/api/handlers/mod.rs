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
    deny_request, get_audit_event_types, get_audit_log, grant_request, list_capabilities,
    list_pending, request_capability, request_status, revoke_all_capabilities, revoke_capability,
    session_info, CapabilityState,
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
