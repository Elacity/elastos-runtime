//! Typed Vz error + exit-reason surface.
//!
//! Vz lifecycle errors cross the ComputeProvider trait as
//! `ElastosError::Compute(String)`, but the macOS backend also
//! keeps a typed surface for structured telemetry. Operators
//! piping `elastos status` JSON into Datadog / Grafana can
//! distinguish "guest panic mid-stop", "VZErrorOperationCancelled
//! during start-then-stop", and "we hit our own
//! [`VzConfig::stop_timeout`][crate::VzConfig::stop_timeout] and
//! force-orphaned the handle" without grepping a free-form
//! `Compute` message.
//!
//! Public surface:
//!
//! - [`VzError`]: typed classification of Apple Vz framework
//!   failures, plus our own synthetic [`VzError::TimedOut`]
//!   variant for the stop-timeout case.
//! - [`VzExitReason`]: typed classification of a successful
//!   terminal state — host-initiated stop, guest clean stop,
//!   stopped-with-error (delegate observed), or
//!   forced-after-timeout (we orphaned the handle).
//!
//! Both types live in the **public** surface of [`elastos_vz`]
//! so the supervisor can pattern-match without re-parsing
//! strings. The existing `ElastosError::Compute(String)` arm in
//! the (Linux-untouched) [`elastos_common`] error type is
//! preserved at the trait boundary; the Vz backend exposes typed
//! data via [`crate::RunningVm`] APIs and via the
//! [`Display`] / [`std::error::Error`] impls.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Typed classification of a Vz failure. Mirrors the variants of
/// Apple's `VZErrorCode` for the codes this backend classifies,
/// matrix called out, plus a synthetic [`Self::TimedOut`] for
/// the local stop-timeout path, plus a forward-compatible
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
    OperationCancelled { description: String },
    /// `VZErrorNotSupported` (10) — operation not available on
    /// this host (entitlement, OS version, hardware).
    NotSupported { description: String },
    /// Synthetic — we hit our own
    /// [`crate::VzConfig::stop_timeout`] before Apple's
    /// completion handler fired. The Vz handle is best-effort
    /// orphaned; the supervisor continues with overlay cleanup.
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
                 continue with overlay cleanup. See docs/MAC.md."
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

impl VzError {
    /// Project this typed error into the operator-facing
    /// [`VzErrorReport`] surface — the JSON shape the supervisor
    /// returns from the `CapsuleVzError` RPC.
    ///
    /// Fields are populated only when meaningful for the
    /// variant:
    ///
    /// - **`kind_label` / `description`** are always present
    ///   (mirror [`Self::kind_label`] / [`Self::description`]).
    /// - **`domain` / `code`** are populated only for
    ///   [`Self::Unknown`] — that's the only variant where the
    ///   raw Apple identifiers carry information the typed
    ///   variants haven't already absorbed. Operators can grep
    ///   on `code=N` to filter Apple variants the binding
    ///   doesn't yet model.
    /// - **`vm_id` / `budget_secs`** are populated only for
    ///   [`Self::TimedOut`] — operator alerting on "stop budget
    ///   too tight per-fleet" needs the budget value, not the
    ///   description text.
    pub fn to_report(&self) -> VzErrorReport {
        let mut report = VzErrorReport {
            kind_label: self.kind_label().to_string(),
            description: self.description(),
            domain: None,
            code: None,
            vm_id: None,
            budget_secs: None,
        };
        match self {
            VzError::Unknown {
                domain,
                code,
                description: _,
            } => {
                report.domain = Some(domain.clone());
                report.code = Some(*code);
            }
            VzError::TimedOut { vm_id, budget } => {
                report.vm_id = Some(vm_id.clone());
                report.budget_secs = Some(budget.as_secs_f64());
            }
            // Documented Apple variants leave domain/code/vm_id
            // implicit — the kind_label IS the structured signal.
            VzError::Internal { .. }
            | VzError::InvalidConfiguration { .. }
            | VzError::InvalidState { .. }
            | VzError::InvalidStateTransition { .. }
            | VzError::NetworkError { .. }
            | VzError::OperationCancelled { .. }
            | VzError::NotSupported { .. } => {}
        }
        report
    }
}

/// Operator-facing JSON projection of a [`VzError`].
///
/// Carried via the new
/// `SupervisorResponse::vz_error: Option<VzErrorReport>` field
/// on both the [`capsule_vz_error`][rpc] RPC and the
/// `capsule_status` enrichment for stopped Vz capsules.
///
/// Every field except `kind_label` + `description` is
/// optional and `#[serde(skip_serializing_if = "Option::is_none")]`
/// so the JSON shape stays minimal for the common case and
/// dashboards can rely on field presence as a typed signal
/// (e.g. presence of `code` means the supervisor saw a future /
/// unmodelled Apple variant).
///
/// [rpc]: # "elastos-server/src/supervisor.rs::Supervisor::capsule_vz_error"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VzErrorReport {
    /// Stable telemetry label — one of [`VzError::kind_label`]'s
    /// outputs (e.g. `"vz_internal"`, `"vz_timed_out"`,
    /// `"vz_unknown"`). Dashboards / alerts MUST filter on this
    /// rather than on `description`.
    pub kind_label: String,
    /// Localised description from Apple's
    /// `NSError.localizedDescription`, or — for
    /// [`VzError::TimedOut`] — the synthesised operator runbook
    /// string. Free-form text; do NOT use for alerting.
    pub description: String,
    /// Raw `NSError.domain` for [`VzError::Unknown`]; `None` for
    /// every documented variant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// Raw `NSError.code` for [`VzError::Unknown`]; `None` for
    /// every documented variant. Operators can filter on
    /// specific Apple codes the typed enum doesn't yet model
    /// (e.g. a future `VZErrorVirtualMachineGuestPaniced`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<isize>,
    /// VM identifier for [`VzError::TimedOut`] — matches the
    /// supervisor's `handle` log lines so operators can pivot
    /// from the alert to the surrounding logs without parsing
    /// the description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vm_id: Option<String>,
    /// Configured stop-timeout budget (seconds, as `f64` so
    /// sub-second budgets survive the JSON wire) for
    /// [`VzError::TimedOut`]. Operators sizing the fleet-wide
    /// `VzConfig::stop_timeout` use this directly.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget_secs: Option<f64>,
}

/// Typed classification of a Vz VM's terminal state. Mirrors
/// [`super::ffi::delegate::DelegateExit`] (which is `pub(crate)`
/// for FFI hygiene) so the supervisor — and any external
/// consumer of [`crate::RunningVm`] — can pattern-match without
/// touching the FFI types.
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
    /// best-effort orphaned.
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
        // Each documented code must round-trip into a typed variant whose
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
        // The typed timeout message must include the vm_id (log
        // correlation), the budget
        // (operator can confirm the configured value), and a
        // runbook pointer (so a search for "vz stop timed out"
        // finds the doc, not just the log line).
        let err = VzError::TimedOut {
            vm_id: "vz-timeout-test".into(),
            budget: Duration::from_millis(750),
        };
        let desc = err.description();
        assert!(
            desc.contains("vz-timeout-test"),
            "must include vm_id: {desc}"
        );
        assert!(desc.contains("750ms"), "must include budget: {desc}");
        assert!(
            desc.contains("docs/MAC.md"),
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
        // in these MUST stay in sync.
        assert_eq!(VzExitReason::GuestCleanStop.exit_code(), 0);
        assert_eq!(VzExitReason::HostInitiatedStop.exit_code(), 0);
        assert_eq!(VzExitReason::StoppedWithError.exit_code(), 1);
        assert_eq!(VzExitReason::ForcedAfterTimeout.exit_code(), 137);
    }

    /// every documented Apple variant must
    /// produce a report with `domain` / `code` / `vm_id` /
    /// `budget_secs` left `None`. The `kind_label` IS the
    /// structured signal for these — populating the raw Apple
    /// code on a typed variant would be redundant and risk
    /// dashboards filtering on `code=1` instead of
    /// `kind_label="vz_internal"`, breaking when Apple
    /// renumbers (unlikely but possible).
    #[test]
    fn to_report_for_documented_variants_omits_unknown_specific_fields() {
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
            let err = VzError::from_ns_error_parts("VZErrorDomain", *code, "Apple's text");
            let report = err.to_report();
            assert_eq!(report.kind_label, *expected_label);
            assert_eq!(report.description, "Apple's text");
            assert!(
                report.domain.is_none(),
                "documented variant {expected_label} must leave domain implicit: {report:?}"
            );
            assert!(
                report.code.is_none(),
                "documented variant {expected_label} must leave code implicit: {report:?}"
            );
            assert!(
                report.vm_id.is_none(),
                "documented variant {expected_label} must leave vm_id implicit: {report:?}"
            );
            assert!(
                report.budget_secs.is_none(),
                "documented variant {expected_label} must leave budget_secs implicit: {report:?}"
            );
        }
    }

    /// `Unknown` populates `domain` + `code` so
    /// operators can grep specific Apple variants the typed
    /// enum doesn't yet model (e.g. a future
    /// `VZErrorVirtualMachineGuestPaniced`). `vm_id` /
    /// `budget_secs` stay `None`.
    #[test]
    fn to_report_for_unknown_variant_populates_raw_apple_identifiers() {
        let err = VzError::from_ns_error_parts("VZErrorDomain", 30001, "USB controller not found");
        let report = err.to_report();
        assert_eq!(report.kind_label, "vz_unknown");
        assert_eq!(report.description, "USB controller not found");
        assert_eq!(report.domain.as_deref(), Some("VZErrorDomain"));
        assert_eq!(report.code, Some(30001));
        assert!(report.vm_id.is_none());
        assert!(report.budget_secs.is_none());

        // Non-Vz domain (e.g. NSPOSIXErrorDomain) must surface
        // the original domain — operators tracing lower-level
        // OS errors need the domain to make sense of the code.
        let posix = VzError::from_ns_error_parts("NSPOSIXErrorDomain", 13, "Permission denied");
        let posix_report = posix.to_report();
        assert_eq!(posix_report.kind_label, "vz_unknown");
        assert_eq!(posix_report.domain.as_deref(), Some("NSPOSIXErrorDomain"));
        assert_eq!(posix_report.code, Some(13));
    }

    /// `TimedOut` populates `vm_id` +
    /// `budget_secs` from the structured fields, NOT by parsing
    /// the description. Operators sizing fleet-wide
    /// `VzConfig::stop_timeout` query `budget_secs` directly;
    /// alerts pivoting from "forced_after_timeout spike" to
    /// "which capsule" use `vm_id`. `domain` / `code` stay
    /// `None` (no Apple identifiers for the synthetic case).
    #[test]
    fn to_report_for_timed_out_populates_vm_id_and_budget_seconds() {
        let err = VzError::TimedOut {
            vm_id: "vz-report-test".into(),
            budget: Duration::from_millis(1_500),
        };
        let report = err.to_report();
        assert_eq!(report.kind_label, "vz_timed_out");
        assert_eq!(report.vm_id.as_deref(), Some("vz-report-test"));
        assert_eq!(report.budget_secs, Some(1.5));
        assert!(report.domain.is_none());
        assert!(report.code.is_none());
        assert!(
            report.description.contains("vz-report-test"),
            "description still embeds the runbook pointer + vm_id for human readers: {}",
            report.description
        );
    }

    /// the JSON wire format. The new
    /// `VzErrorReport` MUST round-trip through `serde_json`
    /// without losing fields, and optional fields MUST
    /// skip-serialise so dashboards can use field presence as a
    /// typed signal.
    #[test]
    fn to_report_serde_round_trip_preserves_typed_fields_and_skips_none() {
        // Documented variant: only kind_label + description
        // appear in JSON.
        let report =
            VzError::from_ns_error_parts("VZErrorDomain", 1, "internal failure").to_report();
        let json = serde_json::to_string(&report).expect("serialise");
        assert!(
            json.contains("\"kind_label\":\"vz_internal\""),
            "kind_label must appear: {json}"
        );
        assert!(
            !json.contains("\"domain\""),
            "documented variant must skip-serialise `domain`: {json}"
        );
        assert!(
            !json.contains("\"code\""),
            "documented variant must skip-serialise `code`: {json}"
        );
        assert!(
            !json.contains("\"vm_id\""),
            "documented variant must skip-serialise `vm_id`: {json}"
        );
        assert!(
            !json.contains("\"budget_secs\""),
            "documented variant must skip-serialise `budget_secs`: {json}"
        );

        let parsed: VzErrorReport = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(parsed, report);

        // Unknown variant: domain + code appear; vm_id +
        // budget_secs stay skipped.
        let unknown = VzError::Unknown {
            domain: "VZErrorDomain".into(),
            code: 12345,
            description: "future".into(),
        }
        .to_report();
        let unknown_json = serde_json::to_string(&unknown).expect("serialise unknown");
        assert!(unknown_json.contains("\"domain\":\"VZErrorDomain\""));
        assert!(unknown_json.contains("\"code\":12345"));
        assert!(!unknown_json.contains("\"vm_id\""));
        assert!(!unknown_json.contains("\"budget_secs\""));

        // TimedOut variant: vm_id + budget_secs appear; domain
        // + code stay skipped.
        let timed_out = VzError::TimedOut {
            vm_id: "vm-x".into(),
            budget: Duration::from_secs(2),
        }
        .to_report();
        let timed_out_json = serde_json::to_string(&timed_out).expect("serialise timed_out");
        assert!(timed_out_json.contains("\"vm_id\":\"vm-x\""));
        assert!(timed_out_json.contains("\"budget_secs\":2.0"));
        assert!(!timed_out_json.contains("\"domain\""));
        assert!(!timed_out_json.contains("\"code\""));
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
