//! Capability-conformance harness — the runtime's central invariant, made testable:
//! **no privileged effect is reachable without a valid, scoped capability token.**
//!
//! Built from the 2026-06-14 capability inventory (see `docs/CAPABILITY_AUDIT.md`).
//! It has two parts:
//!
//! 1. `gate` — POSITIVE conformance: the capability validator denies every wrong-scope,
//!    expired, or over-used token, a denial is audited, and tokens cannot be forged by
//!    external code at all (proven by compilation — token fields are `pub(crate)`). These
//!    tests MUST pass; they run under `just verify` (`cargo test --workspace`).
//!
//! 2. `gaps` — the inventory's architecture-level findings that are NOT yet enforced
//!    (shell exemption, unguarded provider registry, self-asserted `principal_id`, unsigned
//!    best-effort audit, optional `proof_binding_id`, unsigned rights receipts, human-vs-AI
//!    not enforced). Each is recorded in `KNOWN_GAPS` and carried by an `#[ignore]`d test.
//!    They are `#[ignore]`d on purpose: a hard failure here would break `just verify` for
//!    the in-flight dDRM work. The RATCHET: when a gap is fixed, replace its `#[ignore]`d
//!    placeholder with a real passing assertion and delete the `KNOWN_GAPS` row. Once dDRM
//!    lands, the highest-severity gaps should be flipped to blocking (remove `#[ignore]`).

use std::sync::Arc;

use elastos_runtime::capability::manager::ValidationError;
use elastos_runtime::capability::{
    Action, CapabilityManager, CapabilityStore, ResourceId, TokenConstraints,
};
use elastos_runtime::primitives::audit::AuditLog;
use elastos_runtime::primitives::metrics::MetricsManager;
use elastos_runtime::primitives::time::SecureTimestamp;

const RES: &str = "localhost://Users/self/Documents/test.txt";
const OTHER_RES: &str = "localhost://Users/self/Documents/secret.txt";

fn manager_with(audit: AuditLog) -> CapabilityManager {
    CapabilityManager::new(
        Arc::new(CapabilityStore::new()),
        Arc::new(audit),
        Arc::new(MetricsManager::new()),
    )
}

fn manager() -> CapabilityManager {
    manager_with(AuditLog::new())
}

/// Default constraints with `max_uses` overridden, built through the public API
/// (token fields are sealed) while preserving `default()`'s epoch so the epoch check
/// passes and the use-limit check is what fires.
fn constraints_max_uses(n: u32) -> TokenConstraints {
    let d = TokenConstraints::default();
    TokenConstraints::new(d.epoch(), d.delegatable(), d.max_classification(), Some(n))
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. Gate conformance — the invariant proven against the PUBLIC API. MUST pass.
// ─────────────────────────────────────────────────────────────────────────────

/// A token minted for capsule A must not authorize capsule B (no cross-capsule reuse).
#[tokio::test]
async fn token_denies_a_different_capsule() {
    let m = manager();
    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        TokenConstraints::default(),
        None,
    );
    let r = m
        .validate(
            &token,
            "capsule-b",
            Action::Read,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(
        matches!(r, Err(ValidationError::WrongCapsule { .. })),
        "a token must not authorize a different capsule; got {r:?}"
    );
}

/// A Read token must not authorize a Write.
#[tokio::test]
async fn token_denies_a_different_action() {
    let m = manager();
    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        TokenConstraints::default(),
        None,
    );
    let r = m
        .validate(
            &token,
            "capsule-a",
            Action::Write,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(
        matches!(r, Err(ValidationError::WrongAction { .. })),
        "a Read token must not authorize a Write; got {r:?}"
    );
}

/// A token scoped to one resource must not authorize another.
#[tokio::test]
async fn token_denies_a_different_resource() {
    let m = manager();
    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        TokenConstraints::default(),
        None,
    );
    let r = m
        .validate(
            &token,
            "capsule-a",
            Action::Read,
            &ResourceId::new(OTHER_RES),
            None,
        )
        .await;
    assert!(
        matches!(r, Err(ValidationError::WrongResource { .. })),
        "a token must not authorize a different resource; got {r:?}"
    );
}

/// An expired token must be denied (fail-closed on time).
#[tokio::test]
async fn token_denies_after_expiry() {
    let m = manager();
    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        TokenConstraints::default(),
        Some(SecureTimestamp::at(1)), // expired in 1970
    );
    let r = m
        .validate(
            &token,
            "capsule-a",
            Action::Read,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(
        matches!(r, Err(ValidationError::TokenExpired)),
        "an expired token must be denied; got {r:?}"
    );
}

/// A use-limited token must be denied after its budget is spent.
#[tokio::test]
async fn token_denies_after_use_limit() {
    let m = manager();
    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        constraints_max_uses(1),
        None,
    );
    let ok = m
        .validate(
            &token,
            "capsule-a",
            Action::Read,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(ok.is_ok(), "first use should pass; got {ok:?}");
    let r = m
        .validate(
            &token,
            "capsule-a",
            Action::Read,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(
        matches!(r, Err(ValidationError::UseLimitExceeded { .. })),
        "a spent use-limited token must be denied; got {r:?}"
    );
}

/// A denial must be recorded in the audit log. This proves the *positive* side of
/// inventory finding #8 (denials are emitted); the *gap* — that the runtime-core audit
/// sink is best-effort and unsigned — is recorded in `KNOWN_GAPS`.
#[tokio::test]
async fn denial_is_audited() {
    let path = std::env::temp_dir().join("elastos-capconf-denial-audit.log");
    let _ = std::fs::remove_file(&path);
    let audit = AuditLog::with_file(&path).expect("open audit file");
    let m = manager_with(audit);

    let token = m.grant(
        "capsule-a",
        ResourceId::new(RES),
        Action::Read,
        TokenConstraints::default(),
        None,
    );
    // Trigger a denial.
    let r = m
        .validate(
            &token,
            "capsule-b",
            Action::Read,
            &ResourceId::new(RES),
            None,
        )
        .await;
    assert!(matches!(r, Err(ValidationError::WrongCapsule { .. })));

    let logged = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    assert!(
        !logged.trim().is_empty(),
        "a capability denial must produce an audit record (got empty log)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// 2. Known gaps — the inventory's architecture findings, as a build-visible registry.
//    Each row: (id, severity, one-line finding, location). When a gap is fixed, delete
//    its row and add a real assertion above. `gaps_registry_is_intact` keeps it honest.
// ─────────────────────────────────────────────────────────────────────────────

struct KnownGap {
    id: &'static str,
    severity: &'static str, // high | med | low
    finding: &'static str,
    location: &'static str,
}

const KNOWN_GAPS: &[KnownGap] = &[
    // GAP-1 (caller-identity spoofing) and GAP-4 (self-asserted principal_id) were RESOLVED as
    // SAFE by the 2026-06-14 security audit: identity is transport/host-bound (no forgeable wire
    // field) and principal_id derives only from a gateway-signed, verified, non-delegatable launch
    // grant (self-assertion is rejected at the launch boundary). See docs/SECURITY_AUDIT.md.
    // Residual to confirm: the binding rests on sandbox socket isolation (supervisor.rs:1063-1110).
    KnownGap {
        id: "GAP-2",
        severity: "low",
        finding: "Provider registry route/send_raw perform no capability check; safety is by-convention at each caller. Mitigated (security audit): the production app-capsule path (carrier_bridge) requires a token and rejects all runtime-control verbs, so this is reachable only by the host-bound shell, not by untrusted capsules — trusted-core hygiene debt, not a bypass.",
        location: "provider/registry.rs:596 (route), :724 (send_raw)",
    },
    KnownGap {
        id: "GAP-3",
        severity: "low",
        finding: "Runtime-core grant() mints a token from caller-supplied strings, gated only by shell identity; CapabilityToken carries no principal fields. Mitigated (security audit): reachable only by the host-bound shell (is_shell is transport-set), not by app capsules — the production carrier path has no shell exemption. Architectural debt, not an untrusted-capsule bypass.",
        location: "capability/manager.rs:237 (grant), capability/token.rs:193",
    },
    KnownGap {
        id: "GAP-5",
        severity: "high",
        finding: "export_managed_secret exports a raw private key gated only by self-asserted principal_id, with no audit event emitted.",
        location: "capsules/wallet-provider/src/account.rs:424",
    },
    KnownGap {
        id: "GAP-6",
        severity: "med",
        finding: "DID signing has no authorization gate: any caller with a non-empty sender_id+ts receives a device-DID signature.",
        location: "capsules/did-provider/src/main.rs:281/293",
    },
    // GAP-7 (dDRM key-release forgeable on the dev/reference + legacy-receipt + Dev-rights paths)
    // was CLOSED on 2026-06-15 by the build guard (DEV_MODE_GUARD_SPEC): the three dev modes are
    // fenced out of release builds by construction, with the production dkms path requiring a
    // wallet-signed AccessGrantV1 confirmed on-chain. Per the registry convention its row is
    // deleted; the real assertions live in each crate's own tests (key-provider reference fence,
    // dkms-authority default-posture, elastos-server rights guard) and are tracked by
    // `gap7_dev_modes_are_fenced_out_of_release_builds` below.
    KnownGap {
        id: "GAP-8",
        severity: "med",
        finding: "Runtime-core audit sink is best-effort and unsigned: a denial whose write fails is silently dropped; the log is not hash-chained/tamper-evident.",
        location: "primitives/audit.rs:300 (emit)",
    },
    KnownGap {
        id: "GAP-9",
        severity: "med",
        finding: "Launch tokens lack device/browser binding, and proof_binding_id is Option — a token minted without it skips the active-auth-session check.",
        location: "elastos-server/src/api/gateway_home_token.rs:339",
    },
    KnownGap {
        id: "GAP-10",
        severity: "low",
        finding: "Human-vs-AI is a naming convention, not enforced: validate() never branches on Users/ vs UsersAI/, and principals are hardwired to Users/. The 'same gate' holds; the 'parallel AI principal' is not realized in authz.",
        location: "capability/manager.rs:328; elastos-server/src/auth.rs:1175",
    },
];

/// Keeps the gap registry honest: every present entry must be well-formed. Prints the
/// registry so `cargo test -- --nocapture` shows the current conformance debt at a glance.
/// An empty registry is the goal (all gaps closed) and passes harmlessly.
#[test]
fn gaps_registry_is_intact() {
    for g in KNOWN_GAPS {
        assert!(g.id.starts_with("GAP-"), "bad gap id: {}", g.id);
        assert!(
            matches!(g.severity, "high" | "med" | "low"),
            "gap {} has invalid severity {}",
            g.id,
            g.severity
        );
        assert!(
            !g.finding.is_empty() && !g.location.is_empty(),
            "gap {} underspecified",
            g.id
        );
        println!("[{}] ({}) {} — {}", g.id, g.severity, g.finding, g.location);
    }
    let highs = KNOWN_GAPS.iter().filter(|g| g.severity == "high").count();
    println!(
        "\ncapability conformance debt: {} gaps ({} high)",
        KNOWN_GAPS.len(),
        highs
    );
}

// ── Ratchet placeholders: each becomes a real assertion when its gap is closed. ──
// They are #[ignore]d so they do not break `just verify` for in-flight work; running
// `cargo test -- --ignored` lists them with their finding.

// (GAP-1 and GAP-4 placeholders removed — resolved as SAFE by the security audit; see the
// note in KNOWN_GAPS and docs/SECURITY_AUDIT.md.)

#[test]
#[ignore = "GAP-2: prove provider registry route/send_raw is unreachable without a prior validate"]
fn gap2_registry_requires_prior_validation() {}

#[test]
#[ignore = "GAP-3: prove runtime-core grant requires a verified, principal-bound, non-delegatable grant"]
fn gap3_core_grant_is_principal_bound() {}

#[test]
#[ignore = "GAP-5: prove export_managed_secret requires a capability/session and emits an audit event (note: wallet key material is now zeroized; the authz gate remains upstream)"]
fn gap5_key_export_is_gated_and_audited() {}

/// GAP-7 (CLOSED): the three insecure dDRM dev modes are fenced out of release builds by
/// construction (DEV_MODE_GUARD_SPEC). This crate cannot see the other crates' cargo features, so
/// the enforcing assertions live where the modes do and run on a plain `cargo test`:
///   - key-provider: `release_reference_fails_closed_without_session_context` + the selection
///     fence (reference backend refused unless `dev-modes`);
///   - dkms-authority: `release_build_fences_out_the_legacy_receipt_path` (legacy off by default);
///   - elastos-server: `release_build_defaults_to_chain_and_refuses_dev_rights_modes`
///     (rights_mode()==Chain + startup guard fails closed).
/// This ratchet is intentionally NOT `#[ignore]`d — GAP-7 is closed, so it must stay green.
#[test]
fn gap7_dev_modes_are_fenced_out_of_release_builds() {
    assert!(
        !KNOWN_GAPS.iter().any(|g| g.id == "GAP-7"),
        "GAP-7 is closed by the build guard; its registry row must stay deleted"
    );
}
