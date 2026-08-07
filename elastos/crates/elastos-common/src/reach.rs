//! Core-derived reach — the honest blast-radius (W0).
//!
//! [`ReachDescriptorV1`] is what the runtime COMPUTES about how far an act
//! actually reaches, so the shell's blast-radius halo is rendered from real
//! state rather than a self-rating. It is derived from the capsule's *enforced*
//! capability (its isolation tier and whether it actually holds network/system
//! access) and the affordance's own concrete `(resource, operation)` — never from
//! the self-declared [`AffordanceRisk`], which a capsule can set to anything.
//! Declared risk stays advisory; the core stamps the observed reach and can flag
//! a declaration that *understates* it ([`ReachDescriptorV1::declared_understates_reach`]).
//!
//! Honest by construction: `observed` is false whenever a dimension cannot be
//! pinned (a resource-less or operation-less affordance), so the halo renders
//! "incomplete" instead of a falsely-cool reading.
//!
//! W1 adds the egress-as-capability vocabulary: [`EgressReach::Allowlisted`]
//! (a leashed, host-scoped grant) vs [`EgressReach::Open`] (wide), with the
//! fail-closed [`EgressAllowlist`] matcher. Enforcing the allow-list at the
//! kernel network boundary (the crosvm TAP firewall) is W1b — it needs KVM +
//! `CAP_NET_ADMIN`, so the model lands here and the boundary lands on a host
//! that has them.

use serde::{Deserialize, Serialize};

use crate::manifest::{AffordanceRisk, CapsuleType, Permissions};

/// Schema tag for the v1 reach descriptor.
pub const REACH_DESCRIPTOR_SCHEMA_V1: &str = "elastos.reach.v1";

/// Network egress an act can perform, tightest → broadest. The crucial moat
/// distinction is `Allowlisted` (scoped, leashed) vs `Open` (anywhere): a
/// verified capsule with `Open` egress can be visibly MORE dangerous than one
/// scoped to a handful of hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EgressReach {
    /// No network egress — the capsule holds no NIC and is not a host process.
    None,
    /// Network egress scoped to a bound [`EgressAllowlist`] — leashed, not open.
    Allowlisted,
    /// Unrestricted network egress (a NIC or host process with no reach scope).
    Open,
}

/// A reach-scoped egress capability: the exact hosts and schemes a capsule is
/// permitted to reach (W1). Fail-closed by construction — an empty allow-list
/// permits NOTHING, so "no grant" and "grant nothing" both deny.
///
/// NOTE (W1a scope): this is the egress-capability MODEL and its fail-closed
/// matcher. Enforcing it at the wasmtime/crosvm network boundary (the kernel TAP
/// firewall) and threading a bound allow-list through the launch path is W1b —
/// it needs `/dev/kvm` + `CAP_NET_ADMIN`, which a sandbox CI host lacks, so it is
/// deliberately NOT claimed here rather than faked.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EgressAllowlist {
    /// Exact hostnames the capsule may reach (no wildcards in v1).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// URL schemes the capsule may use (e.g. `https`).
    #[serde(default)]
    pub allowed_schemes: Vec<String>,
}

impl EgressAllowlist {
    /// True when this allow-list grants no reach at all (so egress is denied).
    pub fn is_empty(&self) -> bool {
        self.allowed_hosts.is_empty() && self.allowed_schemes.is_empty()
    }

    /// Fail-closed admission: a `(host, scheme)` is permitted ONLY if the host is
    /// explicitly listed AND the scheme is explicitly listed. An empty list on
    /// either axis denies everything on that axis.
    pub fn permits(&self, host: &str, scheme: &str) -> bool {
        let host = host.trim();
        let scheme = scheme.trim();
        !host.is_empty()
            && !scheme.is_empty()
            && self.allowed_hosts.iter().any(|h| h == host)
            && self.allowed_schemes.iter().any(|s| s == scheme)
    }
}

impl EgressReach {
    /// Resolve the egress reach from whether the capsule holds a net capability
    /// and the reach allow-list bound to it (if any). Fail-closed and honest:
    /// - no net capability                  → `None`
    /// - net capability + non-empty allow   → `Allowlisted` (leashed)
    /// - net capability + no/empty allow    → `Open` (wide — surfaced as the
    ///   dangerous reading, never quietly downgraded)
    pub fn resolve(has_net_capability: bool, allowlist: Option<&EgressAllowlist>) -> Self {
        if !has_net_capability {
            return EgressReach::None;
        }
        match allowlist {
            Some(list) if !list.is_empty() => EgressReach::Allowlisted,
            _ => EgressReach::Open,
        }
    }
}

/// The isolation boundary the capsule runs within — the container the blast is
/// confined to. Ordered loosely tightest → broadest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsolationTier {
    /// Inert data, no execution.
    Data,
    /// WASM sandbox (tightest execution boundary).
    Wasm,
    /// crosvm microVM.
    MicroVm,
    /// Runs as a host process (a carrier service or OCI container) — broadest.
    HostProcess,
}

/// How broad the resource the act touches is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceScope {
    /// A single, specific object.
    Object,
    /// A wildcard within one root (e.g. `elastos://content/*`).
    Collection,
    /// A scheme-level wildcard reaching across roots (e.g. `elastos://*`).
    System,
    /// The affordance declares no resource — scope cannot be pinned.
    Unknown,
}

/// Whether the act can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reversibility {
    /// Read-like: leaves no durable change.
    Reversible,
    /// Spends/destroys/sends — cannot be taken back.
    OneWay,
    /// The operation is not declared or does not map to a known semantics.
    Unknown,
}

/// Core-computed reach of an affordance — the data behind the blast-radius halo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReachDescriptorV1 {
    /// Schema tag ([`REACH_DESCRIPTOR_SCHEMA_V1`]).
    pub schema: String,
    /// Network egress the capsule can perform.
    pub egress: EgressReach,
    /// The sandbox the capsule runs in.
    pub isolation: IsolationTier,
    /// How broad the touched resource is.
    pub scope: ResourceScope,
    /// Whether the act is reversible.
    pub reversibility: Reversibility,
    /// True only when every dimension was pinned from real inputs. False when the
    /// affordance is resource-less or operation-less, so the halo must render
    /// "incomplete" rather than a falsely-cool reading.
    pub observed: bool,
}

impl ReachDescriptorV1 {
    /// Derive reach from the capsule's ENFORCED capability (isolation tier +
    /// whether it holds network/system access) and the affordance's concrete
    /// `(resource, operation)`. The self-declared risk is deliberately NOT an
    /// input.
    pub fn derive(
        capsule_type: CapsuleType,
        permissions: &Permissions,
        resource: Option<&str>,
        operation: Option<&str>,
    ) -> Self {
        let isolation = isolation_of(capsule_type, permissions);
        // A guest NIC OR a host-process carrier service means real egress.
        let egress = if permissions.guest_network || permissions.carrier {
            EgressReach::Open
        } else {
            EgressReach::None
        };
        let scope = scope_of(resource);
        let reversibility = reversibility_of(operation);
        // The capability-level reach (egress + isolation) is always real; the
        // descriptor is "observed" only when the act-level dimensions are pinned
        // too, so a vague affordance is honestly marked incomplete.
        let observed = !matches!(scope, ResourceScope::Unknown)
            && !matches!(reversibility, Reversibility::Unknown);
        Self {
            schema: REACH_DESCRIPTOR_SCHEMA_V1.to_string(),
            egress,
            isolation,
            scope,
            reversibility,
            observed,
        }
    }

    /// True when the act reaches FAR — wide network, irreversible, or
    /// system-wide. Used to flag a declared risk that understates reality.
    pub fn is_far_reaching(&self) -> bool {
        matches!(self.egress, EgressReach::Open)
            || matches!(self.reversibility, Reversibility::OneWay)
            || matches!(self.scope, ResourceScope::System)
    }

    /// Advisory cross-check (the "a clone must lie" detector): true when the
    /// capsule's self-declared risk reads as LOW while the core-observed reach is
    /// FAR. Declared risk stays advisory; this only flags the contradiction so a
    /// projection can surface it — it does not gate.
    pub fn declared_understates_reach(&self, declared: &AffordanceRisk) -> bool {
        let declared_low = matches!(
            declared,
            AffordanceRisk::Read | AffordanceRisk::Write | AffordanceRisk::Launch
        );
        declared_low && self.is_far_reaching()
    }
}

/// The capsule's isolation tier: a host-process carrier service is the broadest
/// regardless of declared type; otherwise it follows the capsule type.
fn isolation_of(capsule_type: CapsuleType, permissions: &Permissions) -> IsolationTier {
    if permissions.carrier {
        return IsolationTier::HostProcess;
    }
    match capsule_type {
        CapsuleType::Data | CapsuleType::Media => IsolationTier::Data,
        CapsuleType::Wasm => IsolationTier::Wasm,
        CapsuleType::MicroVM => IsolationTier::MicroVm,
        // An OCI container shares the host kernel — treat as a host process.
        CapsuleType::Oci => IsolationTier::HostProcess,
    }
}

/// Resource breadth from the declared resource pattern (no resource → Unknown).
fn scope_of(resource: Option<&str>) -> ResourceScope {
    match resource.map(str::trim).filter(|r| !r.is_empty()) {
        None => ResourceScope::Unknown,
        // Scheme-level wildcard like `elastos://*` reaches across roots.
        Some(r) if r.ends_with("://*") => ResourceScope::System,
        // Any other wildcard is a collection within a root.
        Some(r) if r.contains('*') => ResourceScope::Collection,
        Some(_) => ResourceScope::Object,
    }
}

/// Reversibility from the declared operation verb (no/unknown operation →
/// Unknown). Substring match, first decisive hit wins.
fn reversibility_of(operation: Option<&str>) -> Reversibility {
    let Some(op) = operation.map(str::trim).filter(|o| !o.is_empty()) else {
        return Reversibility::Unknown;
    };
    let op = op.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| op.contains(n));
    // Spends, destroys, or sends value/effects outward — cannot be undone.
    if has(&[
        "delete", "remove", "pay", "spend", "send", "transfer", "mint", "burn", "revoke", "sign",
    ]) {
        Reversibility::OneWay
    } else if has(&["read", "get", "list", "query", "view", "preview", "inspect"]) {
        Reversibility::Reversible
    } else {
        Reversibility::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(guest_network: bool, carrier: bool) -> Permissions {
        Permissions {
            carrier,
            guest_network,
            storage: Vec::new(),
            messaging: Vec::new(),
        }
    }

    #[test]
    fn a_sandboxed_read_reaches_nowhere() {
        let reach = ReachDescriptorV1::derive(
            CapsuleType::Wasm,
            &perms(false, false),
            Some("elastos://rights/film-x"),
            Some("read"),
        );
        assert_eq!(reach.egress, EgressReach::None);
        assert_eq!(reach.isolation, IsolationTier::Wasm);
        assert_eq!(reach.scope, ResourceScope::Object);
        assert_eq!(reach.reversibility, Reversibility::Reversible);
        assert!(reach.observed, "a fully-specified affordance is observed");
        assert!(!reach.is_far_reaching());
    }

    #[test]
    fn a_networked_microvm_delete_over_a_collection_reaches_far() {
        let reach = ReachDescriptorV1::derive(
            CapsuleType::MicroVM,
            &perms(true, false),
            Some("elastos://content/*"),
            Some("delete"),
        );
        assert_eq!(reach.egress, EgressReach::Open);
        assert_eq!(reach.isolation, IsolationTier::MicroVm);
        assert_eq!(reach.scope, ResourceScope::Collection);
        assert_eq!(reach.reversibility, Reversibility::OneWay);
        assert!(reach.is_far_reaching());
    }

    #[test]
    fn a_carrier_service_is_a_host_process_with_open_egress() {
        let reach = ReachDescriptorV1::derive(
            CapsuleType::Wasm, // carrier overrides the declared type
            &perms(false, true),
            Some("elastos://system/x"),
            Some("write"),
        );
        assert_eq!(reach.isolation, IsolationTier::HostProcess);
        assert_eq!(reach.egress, EgressReach::Open);
    }

    #[test]
    fn a_scheme_wildcard_is_system_scope() {
        let reach = ReachDescriptorV1::derive(
            CapsuleType::Wasm,
            &perms(false, false),
            Some("elastos://*"),
            Some("read"),
        );
        assert_eq!(reach.scope, ResourceScope::System);
        assert!(reach.is_far_reaching(), "system scope reaches far");
    }

    #[test]
    fn a_resourceless_or_verbless_affordance_is_not_fully_observed() {
        let no_resource =
            ReachDescriptorV1::derive(CapsuleType::Wasm, &perms(false, false), None, Some("read"));
        assert_eq!(no_resource.scope, ResourceScope::Unknown);
        assert!(
            !no_resource.observed,
            "a resource-less affordance is incomplete"
        );

        let no_verb = ReachDescriptorV1::derive(
            CapsuleType::Wasm,
            &perms(false, false),
            Some("elastos://rights/x"),
            None,
        );
        assert_eq!(no_verb.reversibility, Reversibility::Unknown);
        assert!(
            !no_verb.observed,
            "an operation-less affordance is incomplete"
        );
    }

    #[test]
    fn egress_allowlist_is_fail_closed() {
        // Empty allow-list permits nothing.
        let empty = EgressAllowlist::default();
        assert!(empty.is_empty());
        assert!(!empty.permits("api.ela.city", "https"));

        let list = EgressAllowlist {
            allowed_hosts: vec!["api.ela.city".to_string()],
            allowed_schemes: vec!["https".to_string()],
        };
        assert!(
            list.permits("api.ela.city", "https"),
            "listed host+scheme is permitted"
        );
        assert!(
            !list.permits("evil.example", "https"),
            "off-list host is denied"
        );
        assert!(
            !list.permits("api.ela.city", "http"),
            "off-list scheme is denied"
        );
        assert!(!list.permits("", "https"), "empty host is denied");
    }

    #[test]
    fn egress_resolves_none_allowlisted_open_fail_closed() {
        // No net capability → no egress, regardless of any allow-list.
        assert_eq!(EgressReach::resolve(false, None), EgressReach::None);
        let list = EgressAllowlist {
            allowed_hosts: vec!["api.ela.city".to_string()],
            allowed_schemes: vec!["https".to_string()],
        };
        assert_eq!(EgressReach::resolve(false, Some(&list)), EgressReach::None);
        // Net capability scoped by a non-empty allow-list → leashed.
        assert_eq!(
            EgressReach::resolve(true, Some(&list)),
            EgressReach::Allowlisted
        );
        // Net capability with no/empty scope → honestly wide open.
        assert_eq!(EgressReach::resolve(true, None), EgressReach::Open);
        assert_eq!(
            EgressReach::resolve(true, Some(&EgressAllowlist::default())),
            EgressReach::Open
        );
    }

    #[test]
    fn allowlisted_egress_is_not_far_reaching_but_open_is() {
        // The moat distinction: a leashed egress is NOT far; an open one is.
        let leashed = ReachDescriptorV1 {
            schema: REACH_DESCRIPTOR_SCHEMA_V1.to_string(),
            egress: EgressReach::Allowlisted,
            isolation: IsolationTier::MicroVm,
            scope: ResourceScope::Object,
            reversibility: Reversibility::Reversible,
            observed: true,
        };
        assert!(
            !leashed.is_far_reaching(),
            "a scoped (allowlisted) egress is not far-reaching"
        );
        let open = ReachDescriptorV1 {
            egress: EgressReach::Open,
            ..leashed.clone()
        };
        assert!(open.is_far_reaching(), "an open egress is far-reaching");
    }

    #[test]
    fn declared_low_but_far_reaching_is_flagged() {
        // A capsule claims Read (low) but actually has open egress — the lie the
        // halo must expose.
        let reach = ReachDescriptorV1::derive(
            CapsuleType::MicroVM,
            &perms(true, false),
            Some("elastos://content/film-x"),
            Some("get"),
        );
        assert!(
            reach.declared_understates_reach(&AffordanceRisk::Read),
            "declared-low + far-reaching must be flagged"
        );
        // An honestly-declared high-risk affordance is not flagged as understated.
        assert!(!reach.declared_understates_reach(&AffordanceRisk::Payment));
        // A genuinely contained low-risk affordance is not flagged.
        let contained = ReachDescriptorV1::derive(
            CapsuleType::Wasm,
            &perms(false, false),
            Some("elastos://rights/x"),
            Some("read"),
        );
        assert!(!contained.declared_understates_reach(&AffordanceRisk::Read));
    }
}
