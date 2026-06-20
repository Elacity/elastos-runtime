//! Build-visible DDRM / dKMS audit-verdict ratchet.
//!
//! The capability side has `KNOWN_GAPS` (capability_conformance.rs). This is its DDRM counterpart:
//! every audit verdict that lives as prose in `docs/PRE_AUDIT.md` and
//! `docs/PRINCIPLES_CONFORMANCE.md` is recorded here with the load-bearing invariant AND the test (or
//! CI job) that PINS it — so the prose cannot silently drift from the code. `verdicts_registry_is_intact`
//! keeps the table honest; `cargo test -p elastos-server --test ddrm_verdicts -- --nocapture` prints it.
//!
//! HONESTY NOTE: the pinning tests for capsule-crate verdicts live in THEIR crates (decrypt-provider,
//! ddrm-envelope) and run under the `capsules` CI job (`just verify-capsules`), not here — this
//! registry indexes and centralises them, it does not re-execute them. A few verdicts are pinned by a
//! code-structure property or a dedicated CI job rather than a unit test; the `pinning` column says so
//! explicitly. To close/re-open a verdict: update its row (and the cited test) — never just the prose.

/// One audit verdict and the thing that keeps it true.
struct Verdict {
    /// Finding id as used in the audit docs (H-/M-/A-/PRE- series).
    id: &'static str,
    title: &'static str,
    /// `cleared` = safe-by-construction; `resolved` = was a real defect, now fixed;
    /// `resolved-perf` = behaviour-preserving optimisation; `partial` = partly addressed, rest roadmap.
    status: &'static str,
    /// The single load-bearing fact the verdict rests on.
    invariant: &'static str,
    /// What enforces it: a `crate::module::test` path, a `ci-job:` name, or `structural:` + reason.
    pinning: &'static str,
}

const VERDICTS: &[Verdict] = &[
    Verdict {
        id: "H1",
        title: "Forensic watermark anchor is safe-by-construction",
        status: "cleared",
        invariant: "A delegation signature that does not recover to the owner is rejected, so a forged \
                    anchor can never authorize an open or reach an egressed, decrypted frame.",
        pinning: "ddrm-envelope::access::tests::delegation_sig_from_wrong_wallet_fails_closed",
    },
    Verdict {
        id: "M1",
        title: "ECDSA malleability is not exploitable",
        status: "cleared",
        invariant: "No replay/dedup key is signature-bytes-keyed; the grant replay guard keys on \
                    explicit server nonces (delegation_nonce:request_nonce), which malleability cannot alter.",
        pinning: "ddrm-envelope::access::tests::request_replay_is_rejected",
    },
    Verdict {
        id: "M3",
        title: "Untrusted CENC box counts are bounded (fail-closed)",
        status: "resolved",
        invariant: "A trun/senc sample_count above 1<<20 is rejected BEFORE any allocation or read loop, \
                    so a forged count cannot OOM the parser.",
        pinning: "decrypt-provider::cenc::mp4box::alloc_bound_tests::parse_trun_rejects_implausible_sample_count",
    },
    Verdict {
        id: "A7",
        title: "Quorum retries refresh the single-use grant nonce",
        status: "resolved",
        invariant: "A retry sends a freshly-regenerated grant (new request nonce), not the original \
                    single-use one, so the node's replay guard does not reject a legitimate retry.",
        pinning: "elastos-server::api::access_grant::attempt_tests::retry_uses_freshly_regenerated_grant",
    },
    Verdict {
        id: "A2",
        title: "Single-copy in-place CENC decrypt is byte-identical",
        status: "resolved-perf",
        invariant: "Decrypting in place yields output byte-identical to the prior copy-based path.",
        pinning: "decrypt-provider::pq_envelope::tests::pq_hybrid_round_trip_recovers_cek (quorum suite goldens)",
    },
    Verdict {
        id: "A1",
        title: "Gateway runtime-DID is boot-stable memoized",
        status: "resolved-perf",
        invariant: "The memoized DID equals a fresh load and is stable for the process lifetime; auth \
                    correctness is unchanged.",
        pinning: "structural: memoizes load_existing_gateway_runtime_did; exercised by the home-token tests",
    },
    Verdict {
        id: "PRE-1",
        title: "CEK reconstruction is integrity-checked (Byzantine-safe)",
        status: "resolved",
        invariant: "A validly-sealed but wrong-VALUED share fails closed: 3+ shares cross-check, any \
                    published-commitment mismatch fails closed, and a 2-share quorum without a commitment is refused.",
        pinning: "decrypt-provider::tests::sealed_material_v1_quorum_byzantine_share_fails_closed",
    },
    Verdict {
        id: "PRE-3",
        title: "Audit/custody trail is tamper-evident + fail-closed",
        status: "resolved",
        invariant: "The log is hash-chained + ed25519-signed + fsync'd (editing/dropping a record breaks \
                    verify_chain), and the dDRM open emits a fail-closed content_open before serving.",
        pinning: "elastos-runtime::primitives::audit::tests::dropping_a_record_breaks_the_chain",
    },
    Verdict {
        id: "PRE-4",
        title: "Provider action enforcement is central + fail-closed",
        status: "resolved",
        invariant: "An operation with no explicit action mapping requires Admin (fail-closed); the bridge \
                    enforces the REQUIRED action, not the token's own.",
        pinning: "elastos-server::provider_resource::tests::required_action_classifies_operations_and_fails_closed",
    },
    Verdict {
        id: "PRE-5",
        title: "Node-set-id pin is mandatory in release builds",
        status: "resolved",
        invariant: "A release dkms-authority refuses to authorize against a caller-declared node-set; the \
                    unset pin is fenced by a compile_error in release.",
        pinning: "ci-job: dkms-release-invariant (dkms-authority release guard)",
    },
    Verdict {
        id: "PRE-7",
        title: "GF(256) multiply is constant-time (branchless)",
        status: "resolved",
        invariant: "Both secret-dependent branches are replaced by arithmetic masks over a fixed 8-iteration \
                    loop; correctness is unchanged.",
        pinning: "structural: mask-select multiply in ddrm-envelope; correctness exercised by every quorum-combine test",
    },
    Verdict {
        id: "PRE-8",
        title: "effective_now clamps the caller clock in release",
        status: "resolved",
        invariant: "A caller-supplied now is clamped to the node clock in release (it can only SHORTEN its \
                    own window, never push time forward).",
        pinning: "structural: release clamp in dkms-authority::effective_now; covered by the dkms-release-invariant job's build",
    },
    Verdict {
        id: "PRE-2",
        title: "Metadata / access-pattern confidentiality",
        status: "partial",
        invariant: "DONE: the (wallet, content_id) triple is logged only as a non-reversible log_fp \
                    fingerprint. ROADMAP: blinded ids / anonymous-credential authz / PIR / frame padding \
                    to hide the pattern from node operators + the chain RPC (research, by-design boundary today).",
        pinning: "elastos-server::api::viewer_open::tests::log_fp_redacts_sensitive_identifiers (log-redaction half)",
    },
];

const ALLOWED_STATUS: &[&str] = &["cleared", "resolved", "resolved-perf", "partial"];

/// Keeps the verdict registry honest and visible. Every row must be well-formed, and any row that
/// claims a security verdict is settled (`cleared`/`resolved`) must name something that pins it — a
/// test, a CI job, or an explicit structural reason — never an empty or `todo` pin.
#[test]
fn verdicts_registry_is_intact() {
    for v in VERDICTS {
        assert!(!v.id.is_empty(), "a verdict has an empty id");
        assert!(
            ALLOWED_STATUS.contains(&v.status),
            "verdict {} has invalid status {:?}",
            v.id,
            v.status
        );
        assert!(
            !v.title.is_empty() && !v.invariant.is_empty() && !v.pinning.is_empty(),
            "verdict {} is underspecified (title/invariant/pinning)",
            v.id
        );
        let pin = v.pinning.to_ascii_lowercase();
        assert!(
            !pin.contains("todo") && !pin.contains("none") && !pin.contains("tbd"),
            "verdict {} ({}) is marked settled but cites no real pin: {:?}",
            v.id,
            v.status,
            v.pinning
        );
        // A settled SECURITY verdict must name a concrete enforcement mechanism — a test path
        // (`crate::module::test`), a CI job (`ci-job:`), or an explicit code-structure pin
        // (`structural:` + reason an auditor can check) — never bare prose. The registry deliberately
        // makes the test-pinned vs structural distinction visible rather than pretending everything
        // has a unit test (e.g. constant-time GF math and release-build clamps are structural).
        if v.status == "cleared" || v.status == "resolved" {
            assert!(
                pin.contains("::") || pin.starts_with("ci-job:") || pin.starts_with("structural:"),
                "settled verdict {} must cite a test path, ci-job, or structural pin (got {:?})",
                v.id,
                v.pinning
            );
        }
        println!("[{}] ({}) {} — pinned by: {}", v.id, v.status, v.title, v.pinning);
    }
    let partials = VERDICTS.iter().filter(|v| v.status == "partial").count();
    println!("\n{} verdicts; {} partial (roadmap)", VERDICTS.len(), partials);
}
