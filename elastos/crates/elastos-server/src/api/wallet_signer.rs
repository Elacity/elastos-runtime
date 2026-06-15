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

use std::io::{BufRead, BufReader, Write};
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
}

impl WalletSession {
    fn start(bin: &str, base_path: &str) -> Result<Self, String> {
        let mut child = Command::new(bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("spawn wallet-provider ({bin}): {e}"))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let reader = BufReader::new(child.stdout.take().ok_or("no stdout")?);
        let mut session = Self {
            child,
            stdin,
            reader,
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
        writeln!(self.stdin, "{request}").map_err(|e| format!("write wallet request: {e}"))?;
        self.stdin.flush().map_err(|e| format!("flush: {e}"))?;
        let mut line = String::new();
        let n = self
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("read wallet stdout: {e}"))?;
        if n == 0 {
            return Err("wallet-provider exited before answering".to_string());
        }
        let resp: Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("parse wallet response: {e}"))?;
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
        let _ = self.child.wait();
    }
}

impl Drop for WalletSession {
    fn drop(&mut self) {
        // Best-effort: ensure the subprocess is reaped even on the error path.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
