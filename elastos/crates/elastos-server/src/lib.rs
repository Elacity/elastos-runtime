//! ElastOS Server
//!
//! HTTP API, CLI orchestration, and capsule loading for ElastOS.
//! This crate provides the transport layer (HTTP) and binary entry point.
//! The security-critical runtime logic lives in `elastos-runtime`.

pub mod api;
pub mod auth;
pub mod binaries;
pub mod browser_app_hosts;
pub mod carrier;
pub mod carrier_service;
pub mod collaboration_carrier;
pub mod collaboration_config;
pub(crate) mod collaboration_contact_store;
pub(crate) mod collaboration_core;
pub mod collaboration_default_conversation;
/// Device signing authority is deliberately unavailable outside this crate.
///
/// ```compile_fail
/// use elastos_server::collaboration_device_authority::DefaultConversationDeviceAuthority;
/// ```
pub(crate) mod collaboration_delivery;
pub(crate) mod collaboration_device_authority;
pub(crate) mod collaboration_direct_messages;
pub(crate) mod collaboration_discovery;
pub(crate) mod collaboration_discovery_runtime;
pub mod collaboration_network;
pub mod collaboration_presence;
pub mod collaboration_product;
pub(crate) mod collaboration_profile_authority;
pub(crate) mod collaboration_profile_loader;
mod collaboration_profile_updates;
pub mod collaboration_protocol;
pub mod collaboration_startup;
pub(crate) mod collaboration_transport;
pub(crate) mod component;
pub mod content;
pub mod crypto;
pub mod documents;
pub mod esp_binding;
pub mod fetcher;
pub mod gateway_cmd;
pub mod host_lock;
pub mod init;
pub mod inspect_provider;
pub mod ipfs;
pub mod library;
pub mod local_http;
pub mod notifications;
pub mod operator_control;
pub mod ownership;
pub mod provider_resource;
pub mod resource_bridge;
pub mod room_service;
pub mod runtime;
pub mod runtime_control;
pub mod setup;
pub mod shares;
pub mod shell_cmd;
pub mod sources;
pub mod supervisor;
pub mod update;
pub mod vm_provider;
