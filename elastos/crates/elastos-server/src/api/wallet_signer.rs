//! Gateway-side EVM transaction signing through the `wallet-provider` capsule.
//!
//! Anders' rule: the secp256k1 key NEVER leaves the wallet boundary. The gateway does not
//! hold, derive, or see the private key — it only orchestrates the capsule's own
//! request → approve → sign flow and receives back a finished `signed_transaction` (the
//! RLP-encoded, signed bytes). The real RLP + secp256k1 + keccak signer already lives in
//! `wallet-provider` (`sign_eip155_legacy_transaction`, tested); this module is only the
//! glue that drives it from the buy/mint orchestrations.
//!
//! The composition is exact and was designed in: `chain-provider.prepare_transaction`
//! emits an `elastos.chain.unsigned_transaction_intent/v1`, `wallet-provider`'s
//! `transaction_intent` consumes precisely that, and its `sign_approved` emits a
//! `signed_transaction` that `chain-provider.broadcast_transaction` accepts. The gateway
//! never invents transaction fields.
//!
//! macOS note: like the rights gate and the chain reads, the wallet capsule is driven as
//! a host SUBPROCESS here (the same isolation tier macOS uses today, before the Apple
//! `vz` microVM backend). The store lives at `ELASTOS_DDRM_WALLET_BASE` so the managed
//! account is stable across buys; the key stays inside the capsule's encrypted store.

use std::io::{BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};

use serde_json::{json, Value};

use super::rights_authority::env_nonempty;

/// Compile-time dev-tree default for the wallet-provider capsule; override with
/// `ELASTOS_WALLET_PROVIDER_BIN`.
const DEV_WALLET_PROVIDER_BIN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../capsules/wallet-provider/target/debug/wallet-provider"
);

/// A finished signature handed back by the wallet capsule. Carries no key material.
#[derive(Debug, Clone)]
pub struct ManagedSignature {
    /// The RLP-encoded, signed transaction bytes (`0x…`) ready to broadcast.
    pub signed_transaction: String,
    /// The keccak256 transaction hash the capsule computed over the signed bytes.
    pub transaction_hash: String,
    /// The managed account's EVM address — the recovered signer / `from`.
    pub signer: String,
    /// The managed account id (audit/debug).
    pub account_id: String,
}

pub(crate) fn resolve_wallet_bin() -> String {
    env_nonempty("ELASTOS_WALLET_PROVIDER_BIN")
        .unwrap_or_else(|| DEV_WALLET_PROVIDER_BIN.to_string())
}

/// Where the wallet capsule keeps its encrypted managed-key store. Stable per user so the
/// managed account survives across buys; overridable for tests.
pub(crate) fn resolve_wallet_base() -> String {
    if let Some(base) = env_nonempty("ELASTOS_DDRM_WALLET_BASE") {
        return base;
    }
    if let Some(home) = env_nonempty("HOME") {
        return format!("{home}/.elastos-ddrm-wallet");
    }
    std::env::temp_dir()
        .join("elastos-ddrm-wallet")
        .to_string_lossy()
        .into_owned()
}

/// Sign an EVM transaction with a managed account belonging to `principal_id` on chain
/// `chain_id`. The managed account is created idempotently (`create_new: false`) on first
/// use; `build_intent` is called with its address so the caller can assemble the exact
/// `unsigned_transaction_intent/v1` payload (`from` MUST equal that address).
///
/// Drives ONE wallet-capsule session: create_managed_account → request_signature →
/// approve_approval → sign_approved. The approval is granted inline here because the buy
/// orchestration is the operator's explicit opt-in (`ELASTOS_DDRM_BUY_SIGN=wallet`);
/// production routes the pending request to the user's Wallet/Inbox for human consent
/// before signing — the capsule already supports that split.
pub fn sign_with_managed_account<F>(
    principal_id: &str,
    chain_id: u64,
    build_intent: F,
) -> Result<ManagedSignature, String>
where
    F: FnOnce(&str) -> Result<Value, String>,
{
    let bin = resolve_wallet_bin();
    if !Path::new(&bin).is_file() {
        return Err(format!(
            "wallet-provider not found at {bin}; build it with \
             `cargo build --manifest-path capsules/wallet-provider/Cargo.toml` \
             or set ELASTOS_WALLET_PROVIDER_BIN"
        ));
    }
    let base = resolve_wallet_base();
    let chain_namespace = format!("eip155:{chain_id}");

    let mut session = WalletSession::start(&bin, &base)?;

    let account = session.call(&json!({
        "op": "create_managed_account",
        "principal_id": principal_id,
        "chain_namespace": chain_namespace,
        "create_new": false,
    }))?;
    let account_id = account["account"]["account_id"]
        .as_str()
        .ok_or("wallet-provider create_managed_account missing account_id")?
        .to_string();
    let address = account["account"]["address"]
        .as_str()
        .ok_or("wallet-provider create_managed_account missing address")?
        .to_string();

    let intent = build_intent(&address)?;

    let request = session.call(&json!({
        "op": "request_signature",
        "principal_id": principal_id,
        "account_id": account_id,
        "chain_namespace": chain_namespace,
        "intent": "transaction_intent",
        "capsule_id": "system",
        "resource": "elastos://chain/base/broadcast_transaction",
        "reason": "Buy access token",
        "payload": intent,
    }))?;
    let request_id = request["approval_request"]["request_id"]
        .as_str()
        .ok_or("wallet-provider request_signature missing request_id")?
        .to_string();

    // FOOTGUN (council S41 red-team F1): this approve is INLINE and AUTOMATIC — the operator's
    // `ELASTOS_DDRM_BUY_SIGN=wallet` opt-in IS the consent, so it returns as fast as any other op
    // and sits safely under the shared read deadline. It is NOT a human-consent wait. If a future
    // change routes this approve to the user's Wallet/Inbox for real human consent, that leg MUST
    // run OUTSIDE this deadline (or under a much larger one) — a 30s watchdog would otherwise kill
    // the signer the moment a human takes longer than half a minute to approve.
    session.call(&json!({
        "op": "approve_approval",
        "principal_id": principal_id,
        "request_id": request_id,
        "reason": "buy access token (operator opt-in)",
    }))?;

    let signed = session.call(&json!({
        "op": "sign_approved",
        "principal_id": principal_id,
        "request_id": request_id,
    }))?;

    session.shutdown();

    let signed_transaction = signed["signed_transaction"]
        .as_str()
        .ok_or("wallet-provider sign_approved missing signed_transaction")?
        .to_string();
    let transaction_hash = signed["signed_payload"]["transaction_hash"]
        .as_str()
        .ok_or("wallet-provider sign_approved missing transaction_hash")?
        .to_string();

    Ok(ManagedSignature {
        signed_transaction,
        transaction_hash,
        signer: address,
        account_id,
    })
}

/// A live wallet-provider subprocess that holds one init'd session. `call` writes a
/// request line, reads the response line, and unwraps `ok`/`error`.
struct WalletSession {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    /// Set once the deadline watchdog has fired on this session (council S41 guardian F5). A fire
    /// leaves the child dead; poisoning makes every subsequent `exchange` FAIL fast rather than
    /// spin on a dead child. Since Sprint 43 this is DIAGNOSTIC/UX, not money-load-bearing: the
    /// whole wallet session runs strictly before broadcast, so `buy_access` types ANY sign-leg
    /// error `BuyError::PreBroadcast` (⇒ refund) at its call site regardless of whether this
    /// specific marker is present — an unmarked EPIPE from a dead signer refunds just the same.
    poisoned: bool,
}

/// The marker a wallet-provider deadline carries (Sprint 41). The ENTIRE wallet session
/// (init → create_account → request → approve → sign) runs strictly BEFORE any broadcast op, so a
/// deadline on ANY leg means the tx was never signed and so never broadcast. Since Sprint 43 this
/// marker is DIAGNOSTIC TEXT, not a classification key: `buy_access` maps the sign leg to
/// `BuyError::PreBroadcast` by CONSTRUCTION (its call site), so a hung/failed signer refunds
/// whether or not this exact string is present — the mirror of the chain SEND leg, which
/// `buy_access` types `Indeterminate` at the `broadcast_signed_*` call site. The marker just names
/// the session's purpose (obtaining a signature) in the error text.
pub(crate) const WALLET_SIGN_DEADLINE_MARKER: &str = "wallet-provider sign deadline exceeded";

impl WalletSession {
    fn start(bin: &str, base_path: &str) -> Result<Self, String> {
        let mut cmd = Command::new(bin);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        // Own process group (unix) so a sign-leg deadline kill takes the provider AND any helper
        // it spawned — the same discipline the chain/rights conversations use (S41).
        let mut child = crate::api::capsule_watchdog::spawn_grouped(&mut cmd)
            .map_err(|e| format!("spawn wallet-provider ({bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut session = Self {
            child,
            stdin,
            reader,
            poisoned: false,
        };
        let init = json!({ "op": "init", "config": { "base_path": base_path } });
        session
            .exchange(&init)
            .map_err(|e| format!("wallet init: {e}"))?;
        Ok(session)
    }

    fn call(&mut self, request: &Value) -> Result<Value, String> {
        self.exchange(request)
    }

    fn exchange(&mut self, request: &Value) -> Result<Value, String> {
        // A prior watchdog fire on this session left the child dead; refuse to run a further leg
        // under an unmarked EOF error (council S41 guardian F5). Every wallet leg is strictly
        // pre-broadcast, so the marker (⇒ NotCharged/refund) is the correct classification.
        if self.poisoned {
            return Err(format!(
                "{WALLET_SIGN_DEADLINE_MARKER}: session already killed by an earlier deadline; the \
                 tx was NEVER signed (provably not charged)"
            ));
        }
        writeln!(self.stdin, "{request}").map_err(|e| format!("write wallet request: {e}"))?;
        self.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        // Bound THIS read with the shared watchdog (S41). The child is persistent — reaped by
        // `shutdown` on the success path, or by `Drop` on any error path — so arm/disarm per read
        // is safe: the watchdog only ever kills the still-live child, and a fire poisons the
        // session so no later leg runs on the corpse. The read is 4MB length-capped by the shared
        // `read_capsule_line`, so even a firehose wallet is a bounded pre-broadcast error. A
        // deadline is a PRE-broadcast refusal — see WALLET_SIGN_DEADLINE_MARKER.
        let deadline = crate::api::capsule_watchdog::capsule_read_deadline();
        let watchdog =
            crate::api::capsule_watchdog::DeadlineWatchdog::arm(self.child.id(), deadline);
        let read = crate::api::capsule_watchdog::read_capsule_line(&mut self.reader);
        let fired = watchdog.disarm();
        if fired {
            // The watchdog killed the child. Poison the session either way: on the common
            // fired-and-read-Err path AND on the rare fired-but-read-Ok race (a response landed as
            // the kill fired) — in both the child is now dead, so no later leg may run on it.
            self.poisoned = true;
            if let Err(underlying) = &read {
                return Err(format!(
                    "{WALLET_SIGN_DEADLINE_MARKER}: no response within {}s — wallet-provider \
                     killed; the tx was NEVER signed (provably not charged); underlying: \
                     {underlying}",
                    deadline.as_secs()
                ));
            }
        }
        let resp = read?;
        match resp.get("status").and_then(Value::as_str) {
            Some("ok") => Ok(resp.get("data").cloned().unwrap_or(Value::Null)),
            Some("error") => Err(format!(
                "wallet-provider op failed: {}",
                resp.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )),
            _ => Err("wallet-provider returned malformed response".to_string()),
        }
    }

    fn shutdown(&mut self) {
        let _ = writeln!(self.stdin, "{}", json!({ "op": "shutdown" }));
        let _ = self.stdin.flush();
        // BOUNDED reap (council S41 guardian F1): a wallet provider that answered every op and
        // then ignores `shutdown`/EOF must not park the buy/mint thread forever on `wait()` —
        // after `sign_approved` the signature is in hand but broadcast has NOT been attempted, so
        // a hang here would strand a Pending reservation indefinitely. `reap_grouped` group-kills
        // an un-exiting child after a short grace.
        crate::api::capsule_watchdog::reap_grouped(&mut self.child);
    }
}

impl Drop for WalletSession {
    fn drop(&mut self) {
        // Best-effort: ensure the subprocess is reaped even on the error path.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sprint 41 ratchet (the MONEY-CRITICAL leg): a HUNG wallet-provider is killed at the
    /// deadline — `sign_with_managed_account` returns BOUNDED (never parks the buy/mint thread for
    /// the child's lifetime) and the error carries `WALLET_SIGN_DEADLINE_MARKER`. This proves the
    /// LIVE-KILL half (a real hung signer is killed and returns bounded); the money direction (a
    /// sign-leg failure ⇒ PreBroadcast refund) is proven by construction in `buy_authority`
    /// (`a_wallet_sign_timeout_types_the_buy_as_pre_broadcast`), where the sign leg is mapped to
    /// `BuyError::PreBroadcast` at its call site — the marker itself is diagnostic text (S43).
    #[test]
    #[cfg(unix)]
    fn a_hung_wallet_provider_is_killed_and_classified_pre_broadcast() {
        let _g = crate::api::ddrm_env_lock();
        let prior_deadline = std::env::var("ELASTOS_CHAIN_READ_DEADLINE_SECS").ok();
        let prior_bin = std::env::var("ELASTOS_WALLET_PROVIDER_BIN").ok();
        let prior_base = std::env::var("ELASTOS_DDRM_WALLET_BASE").ok();
        std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", "1");

        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("hung-wallet-provider.sh");
        std::fs::write(&stub, "#!/bin/sh\nsleep 300\n").unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var("ELASTOS_WALLET_PROVIDER_BIN", &stub);
        std::env::set_var("ELASTOS_DDRM_WALLET_BASE", dir.path().join("store"));

        let started = std::time::Instant::now();
        let err = sign_with_managed_account("did:elastos:test-principal", 8453, |addr| {
            Ok(json!({ "from": addr }))
        })
        .unwrap_err();

        match prior_deadline {
            Some(v) => std::env::set_var("ELASTOS_CHAIN_READ_DEADLINE_SECS", v),
            None => std::env::remove_var("ELASTOS_CHAIN_READ_DEADLINE_SECS"),
        }
        match prior_bin {
            Some(v) => std::env::set_var("ELASTOS_WALLET_PROVIDER_BIN", v),
            None => std::env::remove_var("ELASTOS_WALLET_PROVIDER_BIN"),
        }
        match prior_base {
            Some(v) => std::env::set_var("ELASTOS_DDRM_WALLET_BASE", v),
            None => std::env::remove_var("ELASTOS_DDRM_WALLET_BASE"),
        }

        assert!(
            started.elapsed() < std::time::Duration::from_secs(15),
            "the SIGN leg is BOUNDED by the deadline, not the child's 300s sleep"
        );
        assert!(
            err.contains(WALLET_SIGN_DEADLINE_MARKER),
            "the error carries the wallet sign-deadline marker: {err}"
        );
        // The END-to-end money direction (a hung signer ⇒ pre-broadcast refund) is now proven by
        // construction in `buy_authority`: the sign leg is mapped to `BuyError::PreBroadcast` at
        // its call site, ratcheted by `a_wallet_sign_timeout_types_the_buy_as_pre_broadcast`. This
        // test proves the other half — that a hung signer actually returns bounded with the marker.
    }
}
