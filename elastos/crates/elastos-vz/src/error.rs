//! Typed Vz error + exit-reason surface — **Phase 4 Day 7**.
//!
//! Days 1–6 wired Vz lifecycle errors as `Result<_, String>` at
//! the FFI boundary and `ElastosError::Compute(String)` at the
//! [`crate::RunningVm`] boundary. That was fine for human logs
//! but useless for structured telemetry — operators piping
//! `elastos status` JSON into Datadog / Grafana could not
//! distinguish "guest panic mid-stop", "VZErrorOperationCancelled
//! during start-then-stop" and "we hit our own
//! [`VzConfig::stop_timeout`][crate::VzConfig::stop_timeout] and
//! force-orphaned the handle" without grepping the stringly
//! formatted `Compute` message.
//!
//! Day 7 introduces:
//!
//! - [`VzError`]: typed classification of Apple Vz framework
//!   failures, plus our own synthetic [`VzError::TimedOut`]
//!   variant for the stop-timeout case Day 6 added.
//! - [`VzExitReason`]: typed classification of a successful
//!   terminal state — host-initiated stop, guest clean stop,
//!   stopped-with-error (delegate observed), or
//!   forced-after-timeout (we orphaned the handle).
//!
//! Both types live in the **public** surface of [`elastos_vz`]
//! so the supervisor can pattern-match without re-parsing
//! strings. The existing `ElastosError::Compute(String)` arm in
//! the (Linux-untouched) [`elastos_common`] error type is
//! preserved at the trait boundary — Day 7 surfaces the typed
//! flavour via the new [`crate::RunningVm`] APIs and via the
//! [`Display`] / [`std::error::Error`] impls.

use std::time::Duration;

/// Typed classification of a Vz failure. Mirrors the variants of
/// Apple's `VZErrorCode` for the codes Day 5's failure-mode
/// matrix called out, plus a synthetic [`Self::TimedOut`] for
/// our Day 6 stop-timeout, plus a forward-compatible
/// [`Self::Unknown`] for codes the `objc2-virtualization`
/// binding does not yet expose (a future macOS revision can
/// then route through the same channel without a breaking enum
/// change).
///
/// Each variant carries the operator-facing localised
/// description from Apple's `NSError.localizedDescription`
/// (preserved as a `String` because the underlying NSString may
/// outlive the `&NSError` borrow). Telemetry consumers should
/// switch on [`Self::kind_label`] rather than on the description
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VzError {
    /// `VZErrorInternal` (1) — generic framework-level failure.
    Internal { description: String },
    /// `VZErrorInvalidVirtualMachineConfiguration` (2) — only
    /// fires during `validateWithError` today, but included for
    /// completeness so the supervisor's classifier never falls
    /// back to `Unknown` for a documented variant.
    InvalidConfiguration { description: String },
    /// `VZErrorInvalidVirtualMachineState` (3) — API call
    /// invoked when the VM is in an incompatible state (e.g.
    /// `stop` before `start`).
    InvalidState { description: String },
    /// `VZErrorInvalidVirtualMachineStateTransition` (4) — Apple
    /// rejected a state change.
    InvalidStateTransition { description: String },
    /// `VZErrorNetworkError` (7) — network-attachment failure
    /// during start / stop.
    NetworkError { description: String },
    /// `VZErrorOperationCancelled` (9) — Apple cancelled the
    /// operation (e.g. `stop` issued while still `Starting`).
    /// **Phase 4 Day 5** failure-mode matrix item.
    OperationCancelled { description: String },
    /// `VZErrorNotSupported` (10) — operation not available on
    /// this host (entitlement, OS version, hardware).
    NotSupported { description: String },
    /// Synthetic — we hit our own
    /// [`crate::VzConfig::stop_timeout`] before Apple's
    /// completion handler fired. The Vz handle is best-effort
    /// orphaned; the supervisor continues with overlay cleanup.
    /// **Phase 4 Day 6**.
    TimedOut { vm_id: String, budget: Duration },
    /// Apple returned an `NSError` whose `code` is not yet
    /// modelled above (e.g. a new variant Apple added in a
    /// macOS we haven't classified). Carries the raw code + the
    /// domain string so logs / telemetry can recognise it.
    Unknown {
        domain: String,
        code: isize,
        description: String,
    },
}

impl VzError {
    /// Stable telemetry label for this variant. Used by the
    /// supervisor's [`SupervisorResponse::last_exit_reason`][cap]
    /// field; tooling / dashboards should filter / alert on these
    /// strings rather than the [`Display`] output (which is
    /// localised + may change wording).
    ///
    /// [cap]: # "elastos-server/src/supervisor.rs"
    pub fn kind_label(&self) -> &'static str {
        match self {
            VzError::Internal { .. } => "vz_internal",
            VzError::InvalidConfiguration { .. } => "vz_invalid_configuration",
            VzError::InvalidState { .. } => "vz_invalid_state",
            VzError::InvalidStateTransition { .. } => "vz_invalid_state_transition",
            VzError::NetworkError { .. } => "vz_network_error",
            VzError::OperationCancelled { .. } => "vz_operation_cancelled",
            VzError::NotSupported { .. } => "vz_not_supported",
            VzError::TimedOut { .. } => "vz_timed_out",
            VzError::Unknown { .. } => "vz_unknown",
        }
    }

    /// Operator-facing localised description (from Apple's
    /// `NSError.localizedDescription`, or — for [`Self::TimedOut`] —
    /// the synthesised runbook string).
    pub fn description(&self) -> String {
        match self {
            VzError::Internal { description }
            | VzError::InvalidConfiguration { description }
            | VzError::InvalidState { description }
            | VzError::InvalidStateTransition { description }
            | VzError::NetworkError { description }
            | VzError::OperationCancelled { description }
            | VzError::NotSupported { description }
            | VzError::Unknown { description, .. } => description.clone(),
            VzError::TimedOut { vm_id, budget } => format!(
                "vz stop timed out after {budget:?} (vm_id='{vm_id}') — Apple's \
                 stopWithCompletionHandler: did not fire within the budget. \
                 The Vz handle is now best-effort orphaned; the supervisor will \
                 continue with overlay cleanup. See docs/vz-backend/PHASE_4_DAY_6_NOTES.md."
            ),
        }
    }

    /// Construct a `VzError` from the three observable pieces of
    /// an `NSError`: the domain (`@"VZErrorDomain"` for Vz
    /// errors), the `code` (matches `VZErrorCode::*` constants),
    /// and the localised description. Extracted as a pure helper
    /// so unit tests can exercise the classifier without
    /// constructing an actual `NSError` (which would require an
    /// `autoreleasepool` and macOS).
    ///
    /// `domain` is matched case-insensitively against
    /// `"VZErrorDomain"`; other domains route to
    /// [`Self::Unknown`] with the domain preserved.
    pub fn from_ns_error_parts(domain: &str, code: isize, description: &str) -> Self {
        if !domain.eq_ignore_ascii_case("VZErrorDomain") {
            return VzError::Unknown {
                domain: domain.to_string(),
                code,
                description: description.to_string(),
            };
        }
        match code {
            1 => VzError::Internal {
                description: description.to_string(),
            },
            2 => VzError::InvalidConfiguration {
                description: description.to_string(),
            },
            3 => VzError::InvalidState {
                description: description.to_string(),
            },
            4 => VzError::InvalidStateTransition {
                description: description.to_string(),
            },
            7 => VzError::NetworkError {
                description: description.to_string(),
            },
            9 => VzError::OperationCancelled {
                description: description.to_string(),
            },
            10 => VzError::NotSupported {
                description: description.to_string(),
            },
            _ => VzError::Unknown {
                domain: domain.to_string(),
                code,
                description: description.to_string(),
            },
        }
    }
}

impl std::fmt::Display for VzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.kind_label(), self.description())
    }
}

impl std::error::Error for VzError {}

/// Typed classification of a Vz VM's terminal state. Mirrors
/// [`super::ffi::delegate::DelegateExit`] (which is `pub(crate)`
/// for FFI hygiene) so the supervisor — and any external
/// consumer of [`crate::RunningVm`] — can pattern-match without
/// touching the FFI types. **Phase 4 Day 7**.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VzExitReason {
    /// Guest issued `poweroff -h` / `init 0` / similar. Apple's
    /// delegate fired `guestDidStopVirtualMachine:`.
    GuestCleanStop,
    /// Host called [`crate::RunningVm::stop`] and Apple's
    /// `stopWithCompletionHandler:` block resolved cleanly.
    HostInitiatedStop,
    /// Apple's delegate fired
    /// `virtualMachine:didStopWithError:` — the VM tore itself
    /// down because of a framework-level fault.
    StoppedWithError,
    /// Host called [`crate::RunningVm::stop`] but the completion
    /// block did not fire within
    /// [`crate::VzConfig::stop_timeout`]. The Vz handle is
    /// best-effort orphaned. **Phase 4 Day 6**.
    ForcedAfterTimeout,
}

impl VzExitReason {
    /// Stable telemetry label, used by the supervisor's
    /// `last_exit_reason` JSON field. Operators / dashboards
    /// should filter on these strings rather than on the
    /// [`Display`] output (which may be localised in the future).
    pub fn label(&self) -> &'static str {
        match self {
            VzExitReason::GuestCleanStop => "guest_clean_stop",
            VzExitReason::HostInitiatedStop => "host_initiated_stop",
            VzExitReason::StoppedWithError => "stopped_with_error",
            VzExitReason::ForcedAfterTimeout => "forced_after_timeout",
        }
    }

    /// Map to the integer exit code the supervisor surfaces via
    /// `elastos status` / `wait_capsule`. Mirrors Linux's
    /// `128 + SIGKILL(9) = 137` convention for forced
    /// terminations so operator tooling reads identically across
    /// substrates.
    pub fn exit_code(&self) -> i32 {
        match self {
            VzExitReason::GuestCleanStop | VzExitReason::HostInitiatedStop => 0,
            VzExitReason::StoppedWithError => 1,
            VzExitReason::ForcedAfterTimeout => 137,
        }
    }
}

impl std::fmt::Display for VzExitReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_ns_error_parts_maps_documented_vzerror_codes_to_typed_variants() {
        // Walk the failure-mode matrix from Day 5 + the
        // additional codes Day 7 classifies. Each documented
        // code MUST round-trip into a typed variant whose
        // `kind_label` matches the stable telemetry token.
        let cases: &[(isize, &str)] = &[
            (1, "vz_internal"),
            (2, "vz_invalid_configuration"),
            (3, "vz_invalid_state"),
            (4, "vz_invalid_state_transition"),
            (7, "vz_network_error"),
            (9, "vz_operation_cancelled"),
            (10, "vz_not_supported"),
        ];
        for (code, expected_label) in cases {
            let err = VzError::from_ns_error_parts("VZErrorDomain", *code, "Apple's message");
            assert_eq!(
                err.kind_label(),
                *expected_label,
                "VZErrorCode({code}) must map to '{expected_label}', got: {err:?}"
            );
            assert_eq!(
                err.description(),
                "Apple's message",
                "description must round-trip from NSError.localizedDescription"
            );
        }
    }

    #[test]
    fn from_ns_error_parts_maps_unknown_vz_codes_to_unknown_variant() {
        // Codes Apple adds in future macOS revisions (e.g. >30000)
        // must surface as `Unknown` with the original code
        // preserved, so logs / telemetry don't lose information
        // even before we update the enum. The
        // objc2-virtualization binding currently caps at 30004
        // (`VZErrorDeviceNotFound`); we pick a higher number to
        // guard against a future revision.
        let err = VzError::from_ns_error_parts("VZErrorDomain", 99999, "Future Apple error");
        match err {
            VzError::Unknown {
                domain,
                code,
                description,
            } => {
                assert_eq!(domain, "VZErrorDomain");
                assert_eq!(code, 99999);
                assert_eq!(description, "Future Apple error");
            }
            other => panic!("expected VzError::Unknown, got {other:?}"),
        }
    }

    #[test]
    fn from_ns_error_parts_routes_non_vz_domain_to_unknown_preserving_domain() {
        // Apple's docs explicitly mention that the VM framework
        // can surface errors from lower-level domains. Those
        // MUST classify as Unknown with the domain string
        // preserved so operators can still trace them.
        let err = VzError::from_ns_error_parts("NSPOSIXErrorDomain", 13, "Permission denied");
        match err {
            VzError::Unknown { domain, .. } => assert_eq!(domain, "NSPOSIXErrorDomain"),
            other => panic!("expected VzError::Unknown for non-Vz domain, got {other:?}"),
        }
        // Domain matching is case-insensitive on the Vz side so
        // a slightly-off domain string (e.g. from a debug build)
        // still routes correctly.
        let err_lower =
            VzError::from_ns_error_parts("vzerrordomain", 1, "internal failure (lowercase domain)");
        assert_eq!(err_lower.kind_label(), "vz_internal");
    }

    #[test]
    fn timed_out_description_embeds_vm_id_budget_and_runbook_pointer() {
        // The Day 6 contract: the typed timeout message MUST
        // include the vm_id (log correlation), the budget
        // (operator can confirm the configured value), and a
        // runbook pointer (so a search for "vz stop timed out"
        // finds the doc, not just the log line).
        let err = VzError::TimedOut {
            vm_id: "phase4-day7-test".into(),
            budget: Duration::from_millis(750),
        };
        let desc = err.description();
        assert!(
            desc.contains("phase4-day7-test"),
            "must include vm_id: {desc}"
        );
        assert!(desc.contains("750ms"), "must include budget: {desc}");
        assert!(
            desc.contains("PHASE_4_DAY_6_NOTES.md"),
            "must include runbook pointer: {desc}"
        );
        assert_eq!(err.kind_label(), "vz_timed_out");
    }

    #[test]
    fn vz_exit_reason_labels_and_exit_codes_are_stable() {
        // Telemetry consumers depend on the label strings;
        // changes here ARE breaking-changes for any dashboard
        // that filters on them. Pin them.
        assert_eq!(VzExitReason::GuestCleanStop.label(), "guest_clean_stop");
        assert_eq!(
            VzExitReason::HostInitiatedStop.label(),
            "host_initiated_stop"
        );
        assert_eq!(VzExitReason::StoppedWithError.label(), "stopped_with_error");
        assert_eq!(
            VzExitReason::ForcedAfterTimeout.label(),
            "forced_after_timeout"
        );

        // Exit codes match the convention DelegateExit established
        // in Day 6 — these MUST stay in sync.
        assert_eq!(VzExitReason::GuestCleanStop.exit_code(), 0);
        assert_eq!(VzExitReason::HostInitiatedStop.exit_code(), 0);
        assert_eq!(VzExitReason::StoppedWithError.exit_code(), 1);
        assert_eq!(VzExitReason::ForcedAfterTimeout.exit_code(), 137);
    }

    #[test]
    fn vz_error_display_includes_kind_label_for_log_grep() {
        // Operators grep for `vz_internal` / `vz_timed_out`
        // etc. in logs. Display MUST include the stable label
        // verbatim — not just the localised description.
        let err = VzError::from_ns_error_parts("VZErrorDomain", 1, "Some Apple message");
        let rendered = format!("{err}");
        assert!(
            rendered.starts_with("vz_internal:"),
            "Display must prefix with the kind_label for grep: {rendered}"
        );
        assert!(rendered.contains("Some Apple message"));
    }
}
