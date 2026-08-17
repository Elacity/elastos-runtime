//! Provider registry for protocol routing
//!
//! Routes resource requests to appropriate providers based on URL scheme:
//! - `localhost://<file-backed-root>/...` -> Local sovereign PC2 state
//! - `elastos://...` -> Decentralized content and service namespaces
//!
//! Providers are registered at startup and can be dynamically added/removed.

pub mod bridge;
mod registry;

/// Runtime-only SECRETS a spawned capsule must never inherit (Sprint 46, council S43 guardian F4 +
/// S46 red-team F1 — P16). Both are consumed exclusively IN-PROCESS by the gateway and passed
/// onward as explicit op ARGUMENTS where needed, so no capsule has a legitimate use for the env
/// copy:
/// - `ELASTOS_DDRM_BUY_SIGNED_TX` — a fully broadcastable signed transaction (the external-
///   signature buy leg). Leaked to a hostile capsule binary it could be broadcast out-of-band.
/// - `ELASTOS_PAYMENT_TOKEN` — the HTTP payment rail's bearer token. A capsule holding it could
///   charge the rail directly.
///
/// ONE list (P5), stripped at EVERY capsule spawn seam: `capsule_watchdog::spawn_grouped`
/// (chain/wallet/rights/content sidecars), [`ProviderBridge::spawn`](bridge::ProviderBridge)
/// (the general provider capsules), the carrier service spawn, and the shell-capsule spawns
/// (main/serve_cmd). Host-TOOL spawns (git, tar, the gateway self re-exec — which legitimately
/// needs its own env) are deliberately NOT stripped. A source-structural guard
/// (`capsule_watchdog::every_command_spawn_site_is_a_known_seam_and_capsule_seams_strip_secrets`)
/// pins every `Command::new` site in the tree onto one of those two classifications, so a new
/// spawn path cannot silently skip the strip. This is a targeted denylist of runtime-only
/// secrets, NOT a full per-capsule env allowlist — that remains the stronger tracked hardening.
pub const RUNTIME_ONLY_SECRETS: &[&str] = &["ELASTOS_DDRM_BUY_SIGNED_TX", "ELASTOS_PAYMENT_TOKEN"];

pub use bridge::{CapsuleProvider, ProviderBridge, ProviderConfig as BridgeProviderConfig};
pub use registry::{
    is_boot_unpinned_sub_name, localhost_delegated_scheme, EntryType, Provider, ProviderByteRange,
    ProviderCarrierInvoker, ProviderCarrierRoute, ProviderError, ProviderInvocation,
    ProviderInvocationTransport, ProviderProgress, ProviderRegistration, ProviderRegistry,
    ProviderStreamOptions, ProviderStreamRead, ProviderStreamSession, ProviderTransfer,
    ResourceAction, ResourceResponse,
};

// Re-export for use by external provider implementations
#[allow(unused_imports)]
pub use registry::{ResourceEntry, ResourceRequest};
