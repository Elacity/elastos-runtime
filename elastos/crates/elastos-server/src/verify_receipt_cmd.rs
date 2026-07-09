//! `elastos verify-receipt` — independently verify a portable [`MandateReceipt`] off-box.
//!
//! This is the counterparty's side of the Flint "admissible receipt": an auditor, insurer, or
//! regulator is handed a `.json` receipt and runs ONE command to learn whether it is authentic and
//! what it does (and does not) prove — with NO runtime, NO daemon, NO network. It wraps
//! [`verify_mandate_receipt`] and maps the verdict to a fail-closed exit code a script can trust:
//!
//! * `0` — AUTHENTIC: `--signer` was pinned and matched, and every structural check passed.
//! * `1` — INVALID: a hash, signature, scope, or set-binding check failed (tampered/forged).
//! * `3` — VALID-BUT-UNAUTHENTICATED: structurally sound but no signer was pinned (or it did not
//!   match). This is NOT a trust decision — a fabricated receipt is structurally valid under its own
//!   key — so the code is deliberately distinct from `0`.
//! * `4` — COULD-NOT-EVALUATE: bad input (missing/unreadable file, malformed JSON, invalid
//!   `--signer`). Kept distinct from `1` so a script never reads "couldn't read the file" as
//!   "the receipt is forged."

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use elastos_runtime::primitives::{
    verify_mandate_receipt, MandateReceipt, MandateReceiptScope, MandateReceiptVerdict,
};

/// Resolve a `--signer` argument to the lowercase hex ed25519 key the verifier pins against. Accepts
/// either a `did:key:z...` identifier (the runtime's canonical principal namespace) or the raw
/// 64-char hex of the 32-byte key. `None` in ⇒ `None` out (a structural-only check).
fn resolve_expected_signer(signer: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = signer else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        // An empty `--signer` (e.g. `--signer $SIG` with SIG unset) must NOT silently degrade to
        // "no pin" — that would let a script believe it pinned when it did not. Fail loudly.
        bail!(
            "--signer was empty; pass a did:key or 64-char hex to authenticate, or omit it entirely \
             for a structural-only check"
        );
    }
    if raw.starts_with("did:key:") {
        let key = elastos_server::carrier::did_to_public_key(raw)
            .with_context(|| format!("not a valid did:key ed25519 identifier: {raw}"))?;
        return Ok(Some(hex::encode(key.as_bytes())));
    }
    let bytes = hex::decode(raw)
        .with_context(|| "--signer is neither a did:key nor valid hex".to_string())?;
    if bytes.len() != 32 {
        bail!(
            "--signer hex must be 32 bytes (64 hex chars), got {}",
            bytes.len()
        );
    }
    Ok(Some(hex::encode(bytes)))
}

/// Render an attacker-controlled string safely for TERMINAL/LOG display: every control character
/// (newline, carriage return, tab, ANSI ESC, DEL, C0/C1) becomes a visible `\u{..}` escape. A
/// mandate receipt is untrusted input until verified, and its `schema` / `signer_public_key_hex` /
/// `token_id` are printed in the human report; without this, a crafted receipt could inject a fake
/// "Verdict: AUTHENTIC" line or ANSI cursor moves to erase the real verdict. The exit code and the
/// `--json` path (serde-escaped) do not depend on this — it protects only the human report.
fn sanitize_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_control() {
            out.push_str(&format!("\\u{{{:04x}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

/// Human-readable name of the receipt's scope (token id sanitized — it is untrusted input).
fn scope_label(scope: &MandateReceiptScope) -> String {
    match scope {
        MandateReceiptScope::Contiguous => "contiguous (chain slice)".to_string(),
        MandateReceiptScope::Capability { token_id } => {
            format!("capability (token {})", sanitize_display(token_id))
        }
    }
}

/// The fail-closed exit code for a verdict (see module docs): `0` authentic, `1` invalid, `3` valid
/// but unauthenticated. Pure so the mapping can be unit-tested without a `process::exit`.
pub fn exit_code(verdict: &MandateReceiptVerdict) -> i32 {
    if !verdict.structurally_valid {
        1
    } else if verdict.authenticated {
        0
    } else {
        3
    }
}

/// Read + parse a receipt file, resolve the pinned signer, and verify. Pure (no printing, no exit) so
/// it is testable; `run_verify_receipt` layers presentation and the process exit on top.
pub fn evaluate(
    path: &Path,
    signer: Option<&str>,
) -> Result<(MandateReceipt, MandateReceiptVerdict)> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading receipt {}", path.display()))?;
    let receipt: MandateReceipt = serde_json::from_str(&raw)
        .with_context(|| format!("parsing receipt {} as JSON", path.display()))?;
    let expected = resolve_expected_signer(signer)?;
    let verdict = verify_mandate_receipt(&receipt, expected.as_deref());
    Ok((receipt, verdict))
}

/// Verify a mandate receipt file and exit with a fail-closed status code (see module docs).
pub fn run_verify_receipt(path: PathBuf, signer: Option<String>, json: bool) -> Result<()> {
    // Bad input (file/JSON/signer) exits 4 — deliberately NOT 1 (cryptographic INVALID), so a
    // counterparty's script never mistakes "couldn't read it" for "it was tampered."
    let (receipt, verdict) = match evaluate(&path, signer.as_deref()) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("verify-receipt: {e:#}");
            std::process::exit(4);
        }
    };
    let code = exit_code(&verdict);

    if json {
        // Emit the full verdict verbatim for programmatic consumers.
        println!("{}", serde_json::to_string_pretty(&verdict)?);
        std::process::exit(code);
    }

    let is_capability = matches!(receipt.scope, MandateReceiptScope::Capability { .. });
    println!("Mandate receipt: {}", path.display());
    println!("  schema:            {}", sanitize_display(&receipt.schema));
    println!("  scope:             {}", scope_label(&receipt.scope));
    println!("  records:           {}", verdict.records);
    println!(
        "  signer:            {}",
        sanitize_display(&verdict.signer_public_key_hex)
    );
    println!("  hashes:            {}", ok_no(verdict.hashes_ok));
    println!("  signatures:        {}", ok_no(verdict.signatures_ok));
    println!("  set binding:       {}", ok_no(verdict.set_binding_ok));
    println!("  scope rule:        {}", ok_no(verdict.scope_ok));
    if is_capability {
        // Linkage and genesis anchoring are N/A for a filtered, mid-chain capability view.
        println!("  chain linkage:     n/a (capability scope)");
        println!("  starts at genesis: n/a (capability scope)");
    } else {
        println!("  chain linkage:     {}", ok_no(verdict.chain_linkage_ok));
        println!("  starts at genesis: {}", yes_no(verdict.starts_at_genesis));
    }

    match code {
        0 => println!("\nVerdict: AUTHENTIC — signer pinned and matched; all checks passed."),
        1 => println!(
            "\nVerdict: INVALID — {}. Do not trust this receipt.",
            first_failure(&verdict)
        ),
        _ => {
            // Structurally valid but not authenticated: explain precisely why.
            match verdict.signer_matches_expected {
                Some(false) => println!(
                    "\nVerdict: UNAUTHENTICATED — the receipt is signed by {}, NOT the pinned \
                     signer. Do not trust it.",
                    sanitize_display(&verdict.signer_public_key_hex)
                ),
                _ => println!(
                    "\nVerdict: UNAUTHENTICATED — structurally valid but NO --signer was pinned. \
                     This is NOT a trust decision: anyone can self-sign a fabricated receipt. \
                     Re-run with --signer <did:key or hex of the issuer you trust>."
                ),
            }
        }
    }
    if code != 1 {
        // Honest scoping of what even an AUTHENTIC receipt does NOT prove, per scope.
        if is_capability {
            println!(
                "  note: a capability receipt proves the shown actions were authorized and that no \
                 HOLDER altered the set; it does not prove a compromised issuer omitted nothing."
            );
        } else {
            println!(
                "  note: a contiguous receipt proves an unbroken run, but records truncated off the \
                 END (after the last shown) need an external head anchor to detect — 'starts at \
                 genesis' only guards the front."
            );
        }
    }

    std::process::exit(code);
}

fn ok_no(v: bool) -> &'static str {
    if v {
        "ok"
    } else {
        "FAILED"
    }
}

fn yes_no(v: bool) -> &'static str {
    if v {
        "yes"
    } else {
        "no"
    }
}

/// The first structural check that failed, for a one-line INVALID explanation.
fn first_failure(v: &elastos_runtime::primitives::MandateReceiptVerdict) -> &'static str {
    if v.error.is_some() {
        // A hard parse/structure error short-circuits everything; surface it.
        return "the receipt is structurally malformed";
    }
    if !v.hashes_ok {
        "a record hash does not recompute (an event was edited)"
    } else if !v.signatures_ok {
        "a record signature does not verify (forged or wrong key)"
    } else if !v.scope_ok {
        "the scope rule failed (foreign/duplicate/reordered record, or a missing/duplicate grant)"
    } else if !v.set_binding_ok {
        "the issuer's set binding does not match (a record was added, dropped, or reordered)"
    } else {
        "a structural check failed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elastos_runtime::capability::token::{Action, ResourceId, TokenId};
    use elastos_runtime::primitives::AuditLog;

    // Emit a signed grant + two uses for one token, export the per-capability receipt to a JSON file,
    // and return (dir, receipt_path, signer_hex).
    fn write_capability_receipt() -> (tempfile::TempDir, PathBuf, String) {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_file(dir.path().join("audit.log")).unwrap();
        let token = TokenId::new();
        let vendor = ResourceId::new("elastos://pay/vendor");
        log.capability_grant(&token, "vm-agent", &vendor, Action::Write, None);
        log.capability_use(&token, "vm-agent", &vendor, Action::Write, true);
        log.capability_use(&token, "vm-agent", &vendor, Action::Write, true);
        let signer = log.verifying_key_hex().unwrap();
        let receipt = log
            .export_mandate_receipt_for_capability(&token.to_string())
            .expect("receipt");
        let path = dir.path().join("receipt.json");
        std::fs::write(&path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();
        (dir, path, signer)
    }

    // Emit a signed grant + a pay-USE carrying a DRM settlement `rail_ref` in the signed
    // `CapabilityUse` — matching the reconciler's confirm-time binding shape (`Action::Execute` on
    // the `elastos://runtime/pay/<payee>` resource; drm_marketplace.rs). Export the per-capability
    // receipt to a JSON file as an operator would; return (dir, receipt_path, signer_hex, rail_ref).
    fn write_drm_settlement_receipt() -> (tempfile::TempDir, PathBuf, String, String) {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::with_file(dir.path().join("audit.log")).unwrap();
        let token = TokenId::new();
        let vendor = ResourceId::new("elastos://runtime/pay/drm-vendor");
        let rail_ref = "drm:tx=0xC0FFEE;op=0xopER;tid=42;price=1000;tok=usdc".to_string();
        log.capability_grant(&token, "vm-shopper", &vendor, Action::Execute, None);
        log.capability_use_with_rail_ref(
            &token,
            "vm-shopper",
            &vendor,
            Action::Execute,
            true,
            Some(rail_ref.clone()),
        );
        let signer = log.verifying_key_hex().unwrap();
        let receipt = log
            .export_mandate_receipt_for_capability(&token.to_string())
            .expect("receipt");
        let path = dir.path().join("drm_receipt.json");
        std::fs::write(&path, serde_json::to_string_pretty(&receipt).unwrap()).unwrap();
        (dir, path, signer, rail_ref)
    }

    fn did_key_for(hex_key: &str) -> String {
        let bytes: [u8; 32] = hex::decode(hex_key).unwrap().try_into().unwrap();
        let pk = iroh::PublicKey::from_bytes(&bytes).unwrap();
        elastos_server::carrier::public_key_to_did(&pk)
    }

    #[test]
    fn pinned_signer_authenticates_and_exits_zero() {
        let (_dir, path, signer) = write_capability_receipt();
        let (_receipt, verdict) = evaluate(&path, Some(&signer)).unwrap();
        assert!(
            verdict.authenticated,
            "hex-pinned to the true signer ⇒ authentic: {verdict:?}"
        );
        assert_eq!(exit_code(&verdict), 0);
    }

    #[test]
    fn did_key_and_hex_signer_resolve_to_the_same_pin() {
        let (_dir, path, signer) = write_capability_receipt();
        let did = did_key_for(&signer);
        // Both spellings of the same key must authenticate identically.
        let (_r1, via_did) = evaluate(&path, Some(&did)).unwrap();
        let (_r2, via_hex) = evaluate(&path, Some(&signer)).unwrap();
        assert!(via_did.authenticated && via_hex.authenticated);
        assert_eq!(resolve_expected_signer(Some(&did)).unwrap(), Some(signer));
    }

    #[test]
    fn no_signer_is_unauthenticated_exit_three() {
        let (_dir, path, _signer) = write_capability_receipt();
        let (_receipt, verdict) = evaluate(&path, None).unwrap();
        assert!(verdict.structurally_valid, "still structurally sound");
        assert!(!verdict.authenticated, "no pin ⇒ NOT a trust decision");
        assert_eq!(exit_code(&verdict), 3);
    }

    #[test]
    fn wrong_signer_is_unauthenticated_exit_three() {
        let (_dir, path, _signer) = write_capability_receipt();
        // A well-formed ed25519 public key that is simply not the receipt's signer (a known test
        // vector). Structurally the pin is valid; it just does not match ⇒ unauthenticated.
        let other = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
        let (_receipt, verdict) = evaluate(&path, Some(other)).unwrap();
        assert_eq!(verdict.signer_matches_expected, Some(false));
        assert_eq!(exit_code(&verdict), 3);
    }

    #[test]
    fn a_holder_trimmed_receipt_is_invalid_exit_one() {
        let (dir, path, signer) = write_capability_receipt();
        // Drop the last record (a use) from the on-disk JSON, as a holder in transit would.
        let mut receipt: MandateReceipt =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        receipt.records.pop();
        let trimmed = dir.path().join("trimmed.json");
        std::fs::write(&trimmed, serde_json::to_string(&receipt).unwrap()).unwrap();
        let (_receipt, verdict) = evaluate(&trimmed, Some(&signer)).unwrap();
        assert!(
            !verdict.set_binding_ok,
            "set binding must catch the keyless trim"
        );
        assert!(!verdict.structurally_valid);
        assert_eq!(exit_code(&verdict), 1);
    }

    #[test]
    fn signer_resolution_rejects_junk_but_accepts_bare_hex() {
        assert!(resolve_expected_signer(Some("not-a-key")).is_err());
        assert!(resolve_expected_signer(Some("did:key:zNOTBASE58!!")).is_err());
        // Omitted pin ⇒ structural-only; but a PRESENT-yet-empty pin must error (never silently
        // degrade to "no pin" and mislead a script that believes it pinned).
        assert_eq!(resolve_expected_signer(None).unwrap(), None);
        assert!(resolve_expected_signer(Some("   ")).is_err());
        let hex_key = "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29";
        assert_eq!(
            resolve_expected_signer(Some(hex_key)).unwrap(),
            Some(hex_key.to_string())
        );
    }

    /// Sprint 45 (the live-money last mile, gate-runnable half): a receipt carrying a DRM
    /// settlement `rail_ref` in its signed `CapabilityUse` — the shape the reconciler binds on
    /// confirmation — is a real ADMISSIBLE artifact through the standalone CLI evaluator: written to
    /// disk as JSON, read back, and verified to AUTHENTIC (exit 0) with the true signer pinned, with
    /// the `drm:tx=` reference surviving the round-trip into the verified receipt. This exercises the
    /// `evaluate` + `exit_code` path an auditor runs, not just the in-memory `verify_mandate_receipt`.
    #[test]
    fn a_drm_settlement_receipt_verifies_authentic_through_the_cli() {
        let (_dir, path, signer, rail_ref) = write_drm_settlement_receipt();
        let (receipt, verdict) = evaluate(&path, Some(&signer)).unwrap();
        assert!(
            verdict.authenticated,
            "the money-path receipt authenticates via the CLI evaluator: {verdict:?}"
        );
        assert_eq!(exit_code(&verdict), 0, "AUTHENTIC ⇒ exit 0");
        let carried = receipt.records.iter().find_map(|r| match &r.event {
            elastos_runtime::primitives::audit::AuditEvent::CapabilityUse {
                rail_ref: rr,
                success: true,
                ..
            } => rr.clone(),
            _ => None,
        });
        assert_eq!(
            carried.as_deref(),
            Some(rail_ref.as_str()),
            "the on-chain settlement reference survives the JSON round-trip into the verified receipt"
        );
    }

    /// Sprint 45: editing the signed settlement reference (an adversary in transit repoints the
    /// buy at a different on-chain tx) is caught — the hash/signature no longer recompute, so the
    /// CLI evaluator returns INVALID (exit 1), never a trusted verdict. A money receipt is
    /// tamper-evident end to end.
    #[test]
    fn a_tampered_drm_rail_ref_is_invalid_through_the_cli() {
        let (dir, path, signer, _rail_ref) = write_drm_settlement_receipt();
        let raw = std::fs::read_to_string(&path).unwrap();
        let tampered = raw.replace("0xC0FFEE", "0xDEADBEEF");
        assert_ne!(
            raw, tampered,
            "the tamper actually changed the settlement tx bytes"
        );
        let tpath = dir.path().join("tampered.json");
        std::fs::write(&tpath, tampered).unwrap();
        let (_receipt, verdict) = evaluate(&tpath, Some(&signer)).unwrap();
        assert!(
            !verdict.hashes_ok,
            "editing the signed rail_ref must break the RECORD HASH recompute (not merely set-binding)"
        );
        assert!(
            !verdict.structurally_valid,
            "a broken hash ⇒ structurally invalid"
        );
        assert_eq!(
            exit_code(&verdict),
            1,
            "a tampered money receipt is INVALID, never trusted"
        );
    }

    #[test]
    fn display_sanitizer_neutralizes_verdict_line_and_ansi_injection() {
        // A crafted receipt tries to inject a fake AUTHENTIC line + ANSI cursor moves via a field
        // the human report prints. After sanitizing, no raw control byte survives to the terminal.
        let attack = "tok\n\nVerdict: AUTHENTIC\u{1b}[2K\u{1b}[A ok";
        let safe = sanitize_display(attack);
        assert!(!safe.contains('\n'), "newlines must not survive");
        assert!(!safe.contains('\u{1b}'), "ANSI ESC must not survive");
        assert!(
            safe.contains("\\u{000a}") && safe.contains("\\u{001b}"),
            "controls shown escaped"
        );
        // A clean value is displayed verbatim (no needless mangling of real hex/schema).
        assert_eq!(
            sanitize_display("elastos.mandate_receipt/v1"),
            "elastos.mandate_receipt/v1"
        );
        let labelled = scope_label(&MandateReceiptScope::Capability {
            token_id: attack.to_string(),
        });
        assert!(
            !labelled.contains('\n'),
            "scope label must not carry a raw newline"
        );
    }
}
