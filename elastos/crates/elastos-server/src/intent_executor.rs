//! The "done" half of the mandate loop: performing a dispatched intent and REPORTING what was
//! actually done, so the reconciliation is an independent observation — not the declaration copied
//! back to itself.
//!
//! Before this, the dispatch act closure minted the affordance receipt from the intent's OWN
//! declared fields, so the reconciliation was structurally always `Matched` and an authorized-but-
//! unperformed intent still produced a "matched" receipt (KNOWN_GAPS G-M6). An [`IntentExecutor`]
//! fixes that: the runtime invokes a REAL executor for the declared method, and the receipt is
//! minted from the executor's [`IntentExecution::Performed`] report. A method with no registered
//! executor — or one that declines — yields [`IntentExecution::Declined`], which the gate reconciles
//! as `Undelivered`. So only an intent a real executor performed AS DECLARED reconciles as
//! `Matched`; a drifting executor `Diverges`; an unperformed one is `Undelivered`.

use std::collections::HashMap;
use std::sync::Arc;

use elastos_runtime::capability::IntentDeclarationV1;
use elastos_runtime::primitives::audit::AuditLog;

/// The canonical resource `runtime.audit_verify` actually reads: the runtime's whole audit chain.
/// The executor reports THIS (not the declared resource), so a mandate mis-scoped to some unrelated
/// resource reconciles `Diverged` (the runtime read the chain, not what was declared) instead of a
/// misleading `Matched` — the receipt names what was truly read.
pub const AUDIT_CHAIN_RESOURCE: &str = "elastos://runtime/audit-chain";

/// The resource namespace `runtime.content_seen` operates on: a content-ACCESS-CHECK reference of
/// the form `elastos://runtime/content-access/<content-id>`. A `content_seen` mandate is scoped to
/// THIS (not the bare content id), so the receipt's `CapabilityUse` — which carries the resource but
/// not the method — honestly reads as "a read of the access-CHECK for <id>", never as "a read of the
/// content itself". The executor answers a RUNTIME-level question (does the audit history record ANY
/// successful access to <id>), which the operator authorizes by granting the check mandate.
pub const CONTENT_ACCESS_CHECK_PREFIX: &str = "elastos://runtime/content-access/";

/// The resource namespace `runtime.notify` delivers into: an operator-Inbox TOPIC of the form
/// `elastos://runtime/inbox/<topic>`. A notify mandate is scoped to ONE topic (AUD-5-safe: a real
/// path segment, never a bare wildcard) with `action = message` — the receipt therefore reads as
/// "a message delivered to inbox topic <topic>", exactly what happened.
pub const INBOX_NOTIFY_PREFIX: &str = "elastos://runtime/inbox/";

/// The resource namespace `runtime.state_put` writes into: a durable agent-state KEY of the form
/// `elastos://runtime/store/<key>`. A state mandate is scoped to ONE key (AUD-5-safe: a real path
/// segment, never a bare wildcard) with `action = write` — the receipt reads as "a write to state
/// key <key>", exactly what happened. The value stored is the declaration's own `input_hash`
/// commitment (no free-text payload; see `agent_store`).
pub const STATE_PUT_PREFIX: &str = "elastos://runtime/store/";

/// The resource namespace `runtime.pay` spends against: a PAYEE scope of the form
/// `elastos://runtime/pay/<payee>`. A pay mandate is scoped to ONE payee (AUD-5-safe: a real path
/// segment, never a bare wildcard) with `action = execute`. The per-payment AMOUNT rides in the
/// declaration's signed `input_hash` (a decimal integer of spend units), so it cannot be tampered
/// after signing; the cap is enforced separately by the [`SpendMeter`], keyed on the acting capsule.
/// The receipt reads as "a payment of <amount> to <payee>", exactly what happened.
pub const PAY_PREFIX: &str = "elastos://runtime/pay/";

/// Topic slugs are rendered by the operator's Inbox UI, so they are held to a tight charset —
/// a mandate must not be able to smuggle markup, control characters, or path tricks into the
/// operator surface through its own scope string.
fn valid_notify_topic(topic: &str) -> bool {
    !topic.is_empty() && topic.len() <= 64 && is_slug(topic)
}

/// A conservative operator-safe slug: `[A-Za-z0-9._-]` only. No whitespace, no control chars, no
/// markup, no path separators — the exact charset that cannot mislead a plain-text Inbox row.
fn is_slug(s: &str) -> bool {
    s.bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

/// A non-empty operator/agent-safe slug, ≤64 chars. Every agent-chosen string that a
/// side-effecting affordance persists or renders to a human is held to this: `intent_id` and the
/// state key. Both are agent-controlled (the signature covers them, but the agent IS the signer,
/// and the envelope gate deliberately does not constrain them), so a side-effecting executor must
/// bound them itself — else an agent with a mandate could sign
/// `intent_id = "URGENT: run revoke-all and enter your seed…"` and phish the operator, or write a
/// megabyte key. Council F1 (Sprint 16): a malformed field DECLINES (⇒ authorized_not_performed).
/// `pub(crate)` so the spend-budget provisioning handler holds the operator-supplied capsule id to
/// the SAME bound the executor holds the acting identities to — one validator, no drift.
pub(crate) fn valid_slug_1_64(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && is_slug(s)
}

/// A hex commitment, ≤64 chars, or empty (a no-argument act). The declaration's `input_hash` is a
/// value COMMITMENT, so hex is its honest shape; bounding it keeps free text out of what a
/// side-effecting affordance persists/renders.
fn valid_hex_0_64(s: &str) -> bool {
    s.len() <= 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The INDEPENDENT result of performing a declared intent. It MUST describe what the executor
/// actually did — never be copied from the declaration by the caller — because the gate reconciles
/// it field-for-field against the declaration to decide `Matched`/`Diverged`/`Undelivered`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentExecution {
    /// The act was performed; these are the fields the executor ACTUALLY acted on.
    Performed {
        capsule: String,
        method_id: String,
        input_hash: String,
        resource: String,
        action: String,
        /// The external-rail reference for an act that settled on a rail (Sprint 34): a
        /// `runtime.pay` DRM buy reports the tx hash + `operative:tokenId` here so the dispatch
        /// pipeline can bind it onto the signed `CapabilityUse` and thus the portable receipt.
        /// `None` for every act with no rail settlement (all non-pay executors, and a pay whose
        /// rail returned no reference).
        rail_ref: Option<String>,
        /// Data the executor EXPLICITLY discloses to the dispatching agent in the response
        /// (Sprint 39: `runtime.market_quote` returns the quoted terms here). `None` everywhere
        /// else BY DESIGN — this opt-in keeps `state_get`'s one-bit discipline structural: an
        /// affordance's report reaches the agent only when its executor says so, never because
        /// the pipeline surfaced the receipt echo wholesale.
        agent_visible_report: Option<String>,
    },
    /// Nothing was performed (no executor for the method, a precondition failed, the target was
    /// absent). Reconciles as `Undelivered` — never a fabricated `Matched`.
    Declined { reason: String },
}

/// Performs a dispatched intent and reports what was actually done. Implementations are runtime
/// trusted-core (registered at startup), never attacker-supplied.
pub trait IntentExecutor: Send + Sync {
    fn execute(&self, intent: &IntentDeclarationV1) -> IntentExecution;
}

/// Why a payment did not (verifiably) happen — the TWO-GENERALS distinction a real rail forces
/// (Sprint 29). The runtime's action on each is different and money-critical:
///
/// - [`NotCharged`](PayError::NotCharged): the charge PROVABLY did not happen (refused before
///   processing, connection never established, order rejected 4xx). The runtime REFUNDS the
///   reserved cap — the `DidNotAct` discipline.
/// - [`Indeterminate`](PayError::Indeterminate): the outcome is UNKNOWN (timeout after send, 5xx,
///   crash mid-flight — the charge MAY have posted). The runtime KEEPS the reservation: refunding
///   against money that may have moved would let real spend exceed the cap (the one unbreakable
///   invariant). The reservation is resolved out-of-band (rail reconciliation via the idempotency
///   key; the operator's lever is the budget).
#[derive(Debug)]
pub enum PayError {
    NotCharged(String),
    Indeterminate(String),
}

/// A RAIL-AGNOSTIC payment sink for the `runtime.pay` affordance (Sprint 27). The runtime enforces
/// the CAP (via the [`SpendMeter`]) before ever calling this; the provider only moves the money on a
/// rail — a virtual card, ACH, a Stripe charge, an on-chain transfer. It is CRYPTOGRAPHY that signs
/// the mandate and the receipt, not CRYPTOCURRENCY: the rail is whatever the deployment wires here.
///
/// CONTRACT — implementors MUST classify honestly, because the runtime refunds ONLY on
/// [`PayError::NotCharged`]: return `Ok(rail_ref)` iff the money PROVABLY moved;
/// `Err(NotCharged)` ONLY when the charge PROVABLY did not happen; anything you cannot prove either
/// way is `Err(Indeterminate)` — never guess "not charged". `idempotency_key` is unique per SIGNED
/// intent (derived from its signature, so it cannot recycle even when an intent_id ages out of the
/// replay window) — a rail-side dedupe key so retries/reconciliation can never double-move money.
pub trait PaymentProvider: Send + Sync {
    fn pay(&self, payee: &str, amount: u64, idempotency_key: &str) -> Result<String, PayError>;

    /// Which ledger rail this provider's payments belong to (Sprint 44). Stamped onto the
    /// `PaymentRecord` at `begin_attempt` so a rail-specific reconciler (e.g. the DRM confirmation
    /// driver) selects its records by this STRUCTURED tag, not by sniffing the rail-controlled
    /// `rail_note`. REQUIRED (no default — council S44 guardian F2 / red-team F1): every provider
    /// MUST declare its rail so "a hostile HTTP endpoint's crafted `drm:tx=` note can never get its
    /// pending DRM-resolved" is a COMPILE-TIME property, not a comment (the S43 type-over-comment
    /// lesson). Return [`Unknown`](crate::payment_ledger::PaymentRail::Unknown) only for a provider
    /// whose pendings are never reconciled by a rail-specific driver.
    fn rail(&self) -> crate::payment_ledger::PaymentRail;
}

/// A test/demo payment sink: records every payment and always succeeds, returning a deterministic
/// reference. Real deployments swap in [`HttpPaymentProvider`] (or any [`PaymentProvider`]);
/// nothing else about the affordance changes (the cap + receipt live in the runtime, not the rail).
#[derive(Default)]
pub struct MockPaymentProvider {
    pub payments: std::sync::Mutex<Vec<(String, u64)>>,
}

impl PaymentProvider for MockPaymentProvider {
    fn rail(&self) -> crate::payment_ledger::PaymentRail {
        // A test/demo sink that always succeeds (never records a Pending) — no rail-specific
        // reconciler ever polls its records, so Unknown is correct.
        crate::payment_ledger::PaymentRail::Unknown
    }

    fn pay(&self, payee: &str, amount: u64, _idempotency_key: &str) -> Result<String, PayError> {
        let mut log = match self.payments.lock() {
            Ok(l) => l,
            Err(poisoned) => poisoned.into_inner(),
        };
        log.push((payee.to_string(), amount));
        Ok(format!("mock-txn:{payee}:{amount}"))
    }
}

/// The REAL rail connector (Sprint 29): POSTs a payment order to a deployment-configured HTTPS
/// endpoint (a payment service, or a thin adapter in front of Stripe/ACH/a treasury system) and
/// classifies the outcome under the two-generals contract:
///
/// - `2xx` ⇒ `Ok(rail_ref)` — the endpoint CONFIRMED the charge (body is the rail reference).
/// - `4xx` ⇒ `NotCharged` — the endpoint REJECTED the order before processing. This is a contract
///   REQUIREMENT on the endpoint: it must never return 4xx for an order it (may have) processed.
/// - connect/DNS failure (request never sent) ⇒ `NotCharged` — provably nothing reached the rail.
/// - timeout, `5xx`, or any post-send ambiguity ⇒ `Indeterminate` — the charge may have posted;
///   the runtime keeps the reservation and the idempotency key makes reconciliation/retry safe.
///
/// The order carries `{payee, amount, idempotency_key}` as JSON plus an `Idempotency-Key` header;
/// auth is a static bearer token. The call runs on a dedicated OS thread (isolating the blocking
/// client and its panics; the caller still waits — the pay closure's in-flight bound is what
/// protects the runtime). Redirects are never followed (3xx ⇒ indeterminate).
///
/// ENDPOINT CONTRACT edge cases an implementor must know (council S29 F11): ALL 2xx — including
/// `202 Accepted` — read as CHARGED (do not return 202 for a queued-but-unconfirmed order); ALL
/// 4xx — including `408`/`429` — read as NOT charged and REFUND the cap, so never answer 4xx for
/// an order that may have been processed (answer 5xx, which is indeterminate and keeps the
/// reservation).
pub struct HttpPaymentProvider {
    endpoint: String,
    bearer_token: Option<String>,
    timeout: std::time::Duration,
}

/// How long one rail call may block a dispatch worker. Sized against
/// [`MAX_INFLIGHT_PAYMENTS`]: the worst-case wedge with every in-flight slot stuck is
/// `MAX_INFLIGHT_PAYMENTS × RAIL_TIMEOUT` of blocked worker time, so raising either means
/// re-checking the other.
const RAIL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

impl HttpPaymentProvider {
    pub fn new(endpoint: String, bearer_token: Option<String>) -> Self {
        Self {
            endpoint,
            bearer_token,
            timeout: RAIL_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl PaymentProvider for HttpPaymentProvider {
    fn rail(&self) -> crate::payment_ledger::PaymentRail {
        // Positively tag HTTP-rail pendings so the DRM reconciler NEVER polls them — even one whose
        // Indeterminate body a hostile endpoint crafted to begin `drm:tx=` (council S35 RT-F5).
        crate::payment_ledger::PaymentRail::Http
    }

    fn pay(&self, payee: &str, amount: u64, idempotency_key: &str) -> Result<String, PayError> {
        let endpoint = self.endpoint.clone();
        let token = self.bearer_token.clone();
        let timeout = self.timeout;
        let payee = payee.to_string();
        let key = idempotency_key.to_string();
        // A dedicated thread ISOLATES reqwest's blocking client (which refuses to run on an async
        // runtime worker) and its panics — it does NOT free the caller: the join below still
        // blocks the calling thread for up to `timeout` (council S29 F8), which is why the pay
        // closure bounds concurrent in-flight payments fail-closed. A JOIN failure (the thread
        // panicked) is INDETERMINATE — the request may already have been sent.
        //
        // Ambient-proxy trust, stated (council S29 F11): reqwest honors HTTPS_PROXY/HTTP_PROXY, so
        // in a proxied deployment the payment order + bearer token transit the egress proxy — the
        // same trust already extended for every other outbound call from this process.
        let handle = std::thread::spawn(move || -> Result<String, PayError> {
            let client = reqwest::blocking::Client::builder()
                .timeout(timeout)
                // NEVER follow redirects (council S29 F6): the default policy re-issues a 301/302
                // as a GET whose 200 would read as "the endpoint CONFIRMED the charge" — a
                // Performed receipt minted off a login page. A 3xx is classified below.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| PayError::NotCharged(format!("rail client not constructed: {e}")))?;
            let mut req =
                client
                    .post(&endpoint)
                    .header("Idempotency-Key", &key)
                    .json(&serde_json::json!({
                        "payee": payee,
                        "amount": amount,
                        "idempotency_key": key,
                    }));
            if let Some(t) = &token {
                req = req.bearer_auth(t);
            }
            let resp = match req.send() {
                Ok(r) => r,
                Err(e) if e.is_builder() || e.is_connect() => {
                    // A malformed URL (builder) or a connection never established: the order
                    // provably never left this process / reached the rail (council S29 F3 — a
                    // misconfigured endpoint must not read as "the charge may have posted").
                    return Err(PayError::NotCharged(format!("rail unreachable: {e}")));
                }
                Err(e) => {
                    // Timeout or any post-send failure: the order MAY have been processed.
                    return Err(PayError::Indeterminate(format!("rail send ambiguous: {e}")));
                }
            };
            let status = resp.status();
            // Bound the read BEFORE buffering (a misbehaving endpoint must not OOM the money
            // path), then truncate to the reference length actually used.
            let mut body = String::new();
            {
                use std::io::Read as _;
                let _ = resp.take(64 * 1024).read_to_string(&mut body);
            }
            // Sanitize rail-controlled bytes AT SOURCE (council S29 RT-F7): printable ASCII,
            // bounded — before they enter any reason, ledger record, or future signed field.
            let body_head = crate::payment_ledger::sanitize_rail_note(&body);
            if status.is_success() {
                Ok(body_head)
            } else if status.is_redirection() {
                // The order REACHED the endpoint and it answered with a redirect we refuse to
                // follow — whether the real handler processed it is unknowable from here.
                Err(PayError::Indeterminate(format!(
                    "rail redirected ({status}) — refusing to follow for a money order"
                )))
            } else if status.is_client_error() {
                Err(PayError::NotCharged(format!(
                    "rail rejected the order ({status}): {body_head}"
                )))
            } else {
                Err(PayError::Indeterminate(format!(
                    "rail returned {status} after receiving the order: {body_head}"
                )))
            }
        });
        match handle.join() {
            Ok(result) => result,
            Err(_) => Err(PayError::Indeterminate(
                "rail call thread panicked; the order may have been sent".to_string(),
            )),
        }
    }
}

type MethodFn = Arc<dyn Fn(&IntentDeclarationV1) -> IntentExecution + Send + Sync>;

/// A registry mapping `method_id` → an executor. An unregistered method DECLINES (⇒ `Undelivered`),
/// which is the honest default: the runtime performed nothing, so it attests nothing.
#[derive(Clone, Default)]
pub struct MethodRegistryExecutor {
    methods: HashMap<String, MethodFn>,
}

impl MethodRegistryExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The executor set the production runtime ships with. Real affordances are registered here —
    /// each a genuine operation that PERFORMS and reports the action it REALLY did — and only these
    /// methods can reconcile `performed`; every other method DECLINES ⇒ `Undelivered` ⇒
    /// `authorized_not_performed`, the honest state. Registered today:
    ///
    /// - `runtime.audit_verify` — the first real, SIDE-EFFECT-FREE affordance. It re-verifies the
    ///   runtime's own tamper-evident audit chain end to end (hash links + ed25519 signatures) — a
    ///   pure read — and `Performed`s iff the chain actually verifies, `Declined`s otherwise. So the
    ///   outcome tracks REAL chain state, not the declaration: a corrupt or memory-only log is
    ///   honestly `Undelivered`. It reports `action = "read"` (what it truly did), so it is usable
    ///   only under a `read` mandate.
    /// - `runtime.content_seen` — a state-DEPENDENT read: does the audit history record a successful
    ///   access (ContentFetch/ContentOpen) to the mandate's `resource` (a content id)? `Performed`s
    ///   iff yes, `Declined`s iff not — so the SAME intent reconciles `performed` or
    ///   `authorized_not_performed` depending on real runtime state, not the declaration. Unlike
    ///   audit_verify the operation is PARAMETERIZED by the declared resource (it searches for that
    ///   id), so echoing it is honest. Reports `action = "read"`.
    /// - `runtime.notify` — the first SIDE-EFFECTING affordance: deliver a message about the act
    ///   into the operator's Inbox (the shell's Inbox app renders it). Registered ONLY when the
    ///   runtime has a `notify_data_dir` (the Inbox store lives there) — without one the method is
    ///   unwired ⇒ `Undelivered`, never a fabricated delivery. `Performed` iff the notification
    ///   write actually LANDED (atomic store write returned Ok); a failed write `Declined`s with
    ///   the true reason. The message content is a FIXED shape built from the signed declaration's
    ///   own fields (no free-text channel — nothing reaches the operator surface that the intent
    ///   signature does not cover), and the topic slug is charset-checked so a mandate's scope
    ///   string cannot smuggle markup into the Inbox. Reports `action = "message"` — usable only
    ///   under a `message` mandate.
    /// - `runtime.state_put` — the SECOND side-effecting affordance: write a durable, readable-back
    ///   agent-state key (`elastos://runtime/store/<key>`), principal-scoped to the acting capsule.
    ///   The stored value is the declaration's `input_hash` COMMITMENT (no free-text payload), so
    ///   nothing is persisted that the intent signature + mandate receipt do not already bind. Same
    ///   discipline as notify: registered only with a `data_dir`; key + input_hash bounded to safe
    ///   shapes before the write; `Performed` iff the atomic write LANDED, else `Declined`. Reports
    ///   `action = "write"` — usable only under a `write` mandate.
    /// - `runtime.state_get` — the READ side of that KV (the pair of state_put): a PRINCIPAL-SCOPED
    ///   ATTESTED VERIFY read of `elastos://runtime/store/<key>` (NOT a value fetch). The declared
    ///   `input_hash` is the value the agent EXPECTS; `Performed` echoes the ACTUAL stored value-hash
    ///   into the reconciliation's field comparison, so the read reconciles `Matched` iff the key
    ///   holds that value, `Diverged` if it holds a DIFFERENT one, and `Declined`
    ///   (⇒ authorized_not_performed) if the key is absent. Honest scope (council F1/F3): the agent
    ///   learns ONE BIT (matched / diverged / absent) — the actual value is NOT returned to it and is
    ///   NOT put on-chain (the declared value isn't either), so a `Matched` is a runtime-live-trust
    ///   attestation of "K = V" (agent-signed declaration correlated with the runtime-signed
    ///   reconciliation), not a `content_seen`-grade signature-attested proof, and a `Diverged`
    ///   reveals only that the guess was wrong, never the truth. A state_get mandate must BIND an
    ///   agent key (council F2 — an unbound state-read is a token-id-gated confidentiality oracle).
    ///   Keyed on the acting capsule ⇒ an agent verifies only its OWN state. Reports `action = "read"`.
    ///   Registered with a `data_dir`.
    ///
    /// The side-effecting affordances + state_get need the runtime data dir (their stores live under
    /// it); a `None` data dir leaves them honestly unwired ⇒ `Undelivered`.
    pub fn production(audit_log: Arc<AuditLog>, data_dir: Option<std::path::PathBuf>) -> Self {
        let mut registry = Self::new();
        if let Some(data_dir) = data_dir {
            let state_dir = data_dir.clone();
            let state_get_dir = data_dir.clone();
            registry.register(
                "runtime.state_put",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The mandate is scoped to a state KEY; the key is the suffix. Outside the
                    // namespace ⇒ Decline.
                    let Some(key) = intent.resource.strip_prefix(STATE_PUT_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!("state_put resource must be {STATE_PUT_PREFIX}<key>"),
                        };
                    };
                    // The key is a durable identifier and appears in operator/agent read-backs, and
                    // the value is a COMMITMENT (hex hash) — bound both before the write, so nothing
                    // free-form is persisted under the mandate.
                    if !valid_slug_1_64(key) {
                        return IntentExecution::Declined {
                            reason: "state_put key must be 1-64 chars of [A-Za-z0-9._-]"
                                .to_string(),
                        };
                    }
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason:
                                "state_put value (input_hash) must be <=64 hex chars (or empty)"
                                    .to_string(),
                        };
                    }
                    // The intent_id is PERSISTED in the entry (attribution), so it is bounded to a
                    // slug like every other agent-chosen string a side-effecting affordance stores —
                    // no unbounded per-entry payload smuggled through the id.
                    if !valid_slug_1_64(&intent.intent_id) {
                        return IntentExecution::Declined {
                            reason: "state_put intent_id must be 1-64 chars of [A-Za-z0-9._-]"
                                .to_string(),
                        };
                    }
                    // The REAL side effect: persist the key. Performed only after the write lands.
                    match crate::agent_store::put_agent_state(
                        &state_dir,
                        &intent.capsule,
                        key,
                        &intent.input_hash,
                        &intent.standing_grant_id,
                        &intent.intent_id,
                    ) {
                        Ok(_version) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // The declared value-hash is genuinely CONSUMED — it is what was written
                            // as the key's value — so echoing it is honest.
                            input_hash: intent.input_hash.clone(),
                            // The key actually written, and the action actually performed: a write.
                            resource: intent.resource.clone(),
                            action: "write".to_string(),
                            rail_ref: None,
                            agent_visible_report: None,
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("state_put could not be persisted: {e}"),
                        },
                    }
                }),
            );
            registry.register(
                "runtime.state_get",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The READ side of the agent-state KV (Sprint 25) — same store namespace as
                    // state_put; the key is the suffix. Outside the namespace ⇒ Decline.
                    let Some(key) = intent.resource.strip_prefix(STATE_PUT_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!("state_get resource must be {STATE_PUT_PREFIX}<key>"),
                        };
                    };
                    if !valid_slug_1_64(key) {
                        return IntentExecution::Declined {
                            reason: "state_get key must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                        };
                    }
                    // An ATTESTED read (like content_seen's boolean check): the declared input_hash
                    // is the value the agent EXPECTS the key to hold, bounded to the same commitment
                    // shape state_put wrote. A read that reconciles Matched PROVES "key K holds V".
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason: "state_get expected-value (input_hash) must be <=64 hex chars (or empty)"
                                .to_string(),
                        };
                    }
                    // PRINCIPAL-SCOPED: get_agent_state keys on the acting capsule, so an agent can
                    // only ever read its OWN state — never another principal's (the per-capsule
                    // isolation the operator-facing list deliberately does NOT have).
                    match crate::agent_store::get_agent_state(&state_get_dir, &intent.capsule, key) {
                        Ok(Some(entry)) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // Echo the ACTUAL stored value-hash for the reconciliation's field
                            // comparison. Reconcile Matches iff the agent declared it correctly (an
                            // attested "K = V"); otherwise Diverges. The gate keeps only the verdict
                            // (the value is NOT surfaced to the agent nor put on-chain — council F1),
                            // so a mismatch yields one bit ("wrong guess"), never a misleading
                            // Matched and never a value leak.
                            input_hash: entry.value_hash,
                            resource: intent.resource.clone(),
                            action: "read".to_string(),
                            rail_ref: None,
                            agent_visible_report: None,
                        },
                        // No such key for this principal ⇒ authorized-but-not-performed (honest:
                        // there is nothing to read), never a fabricated empty value.
                        Ok(None) => IntentExecution::Declined {
                            reason: format!("no state for {}/{key}", intent.capsule),
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("state_get could not read the store: {e}"),
                        },
                    }
                }),
            );
            registry.register(
                "runtime.notify",
                Arc::new(move |intent: &IntentDeclarationV1| {
                    // The mandate is scoped to an inbox TOPIC; the topic is the suffix. A resource
                    // outside this namespace is not a notify target ⇒ Decline.
                    let Some(topic) = intent.resource.strip_prefix(INBOX_NOTIFY_PREFIX) else {
                        return IntentExecution::Declined {
                            reason: format!("notify resource must be {INBOX_NOTIFY_PREFIX}<topic>"),
                        };
                    };
                    if !valid_notify_topic(topic) {
                        return IntentExecution::Declined {
                            reason: "notify topic must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                        };
                    }
                    // Council F1: the intent_id + input_hash reach the OPERATOR's Inbox body, so
                    // they are bounded to operator-safe shapes BEFORE delivery — a malformed field
                    // declines rather than smuggling free text into the operator's trust surface.
                    if !valid_slug_1_64(&intent.intent_id) {
                        return IntentExecution::Declined {
                            reason: "notify intent_id must be 1-64 chars of [A-Za-z0-9._-]"
                                .to_string(),
                        };
                    }
                    if !valid_hex_0_64(&intent.input_hash) {
                        return IntentExecution::Declined {
                            reason: "notify input_hash must be <=64 hex chars (or empty)"
                                .to_string(),
                        };
                    }
                    // The REAL side effect: land the row in the operator's Inbox store. Performed
                    // is reported ONLY after the atomic write returns Ok — a failed delivery is a
                    // Declined (⇒ authorized_not_performed), never a claimed message.
                    match crate::notifications::post_agent_act_notification(
                        &data_dir,
                        &intent.intent_id,
                        &intent.capsule,
                        topic,
                        &intent.input_hash,
                        &intent.standing_grant_id,
                    ) {
                        Ok(()) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // The declared input hash is genuinely CONSUMED — it is written into
                            // the delivered notification body — so echoing it is honest (the same
                            // way content_seen echoes the resource it actually searched).
                            input_hash: intent.input_hash.clone(),
                            // The topic actually delivered to, and the action actually performed:
                            // a message. A mandate scoped elsewhere, or a non-message action,
                            // reconciles Diverged, never a misleading Matched.
                            resource: intent.resource.clone(),
                            action: "message".to_string(),
                            rail_ref: None,
                            agent_visible_report: None,
                        },
                        Err(e) => IntentExecution::Declined {
                            reason: format!("notification could not be delivered: {e}"),
                        },
                    }
                }),
            );
        }
        let content_log = audit_log.clone();
        registry.register(
            "runtime.content_seen",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // The mandate is scoped to a content-ACCESS-CHECK resource; the content id is the
                // suffix. A resource outside this namespace is not a content_seen target ⇒ Decline.
                let Some(content_id) = intent.resource.strip_prefix(CONTENT_ACCESS_CHECK_PREFIX)
                else {
                    return IntentExecution::Declined {
                        reason: format!(
                            "content_seen resource must be {CONTENT_ACCESS_CHECK_PREFIX}<content-id>"
                        ),
                    };
                };
                // Same evidentiary bar as audit_verify: the log must be SIGNED and the chain must
                // VERIFY, so a matched ContentOpen is a signature-attested record an offline editor
                // could not have forged. Then answer PRINCIPAL-SCOPED: did THIS capsule open it?
                let verifying_key = content_log
                    .verifying_key_hex()
                    .and_then(|hex_key| hex::decode(hex_key).ok())
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                    .and_then(|arr| ed25519_dalek::VerifyingKey::from_bytes(&arr).ok());
                let Some(verifying_key) = verifying_key else {
                    return IntentExecution::Declined {
                        reason: "audit chain is unsigned; cannot attest a verified access".to_string(),
                    };
                };
                if content_log.verify_chain(Some(&verifying_key)).is_err() {
                    return IntentExecution::Declined {
                        reason: "audit chain did not verify".to_string(),
                    };
                }
                if content_log.principal_opened_content(&intent.capsule, content_id) {
                    IntentExecution::Performed {
                        capsule: intent.capsule.clone(),
                        method_id: intent.method_id.clone(),
                        input_hash: String::new(), // the search key IS the resource; no other args
                        // The access-CHECK resource actually searched (== declared: parameterized by
                        // it), and the action performed (a read of the audit history). The receipt
                        // therefore names a read of the CHECK, never of the content bytes.
                        resource: intent.resource.clone(),
                        action: "read".to_string(),
                        rail_ref: None,
                        agent_visible_report: None,
                    }
                } else {
                    IntentExecution::Declined {
                        reason: format!("{} did not open {content_id}", intent.capsule),
                    }
                }
            }),
        );
        registry.register(
            "runtime.audit_verify",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // Require a SIGNING KEY: `verify_chain(None)` is a hash-links-only walk, and
                // `record_hash` is a public algorithm — an offline editor could rewrite an unsigned
                // chain and pass. So a `performed` audit_verify must mean SIGNATURE-verified: with no
                // key (memory-only/unsigned log) we Decline rather than over-claim tamper-evidence.
                let verifying_key = audit_log
                    .verifying_key_hex()
                    .and_then(|hex_key| hex::decode(hex_key).ok())
                    .and_then(|bytes| <[u8; 32]>::try_from(bytes.as_slice()).ok())
                    .and_then(|arr| ed25519_dalek::VerifyingKey::from_bytes(&arr).ok());
                let Some(verifying_key) = verifying_key else {
                    return IntentExecution::Declined {
                        reason: "audit chain is unsigned; cannot attest a signature-verified read"
                            .to_string(),
                    };
                };
                match audit_log.verify_chain(Some(&verifying_key)) {
                    Ok(_verified_count) => IntentExecution::Performed {
                        // capsule + method_id are the bound identity the executor acted as/under
                        // (the gate already tied them to the mandate); the fields below are what the
                        // runtime REALLY did, reported independently of the declaration:
                        capsule: intent.capsule.clone(),
                        method_id: intent.method_id.clone(),
                        // audit_verify consumes NO arguments, so the honest args-hash is empty. An
                        // intent that declared some other input_hash reconciles `Diverged`.
                        input_hash: String::new(),
                        // The resource actually read (the whole chain) and the action actually
                        // performed (read) — a mandate scoped elsewhere, or a non-read action,
                        // therefore reconciles `Diverged`, never a misleading `Matched`.
                        resource: AUDIT_CHAIN_RESOURCE.to_string(),
                        action: "read".to_string(),
                        rail_ref: None,
                        agent_visible_report: None,
                    },
                    Err(reason) => IntentExecution::Declined {
                        reason: format!("audit chain did not verify: {reason}"),
                    },
                }
            }),
        );
        registry
    }

    /// Register the `runtime.pay` affordance (Sprint 27) — a SPEND-CAPPED payment. Opt-in (a
    /// chained builder, so the 20+ existing `production()` sites and every test that does not wire
    /// a meter leave pay honestly UNWIRED ⇒ `Undelivered` ⇒ `authorized_not_performed`).
    ///
    /// The mandate scopes WHO may pay WHOM (capsule + `elastos://runtime/pay/<payee>` resource +
    /// action `execute` + agent-key binding, all enforced by the gate BEFORE this runs); the
    /// [`SpendMeter`] caps HOW MUCH, keyed on the acting capsule. Flow, fail-closed throughout:
    /// 1. the payee comes from the resource, the AMOUNT from the signed `input_hash` (decimal units);
    /// 2. `meter.try_debit(capsule, amount)` RESERVES the spend atomically — over the cap (or an
    ///    unprovisioned capsule ⇒ zero budget) it refuses and NOTHING is debited or paid: the act
    ///    Declines and the chain records it as `authorized_not_performed` (a SIGNED refusal that no
    ///    payment happened; the specific decline REASON is not yet on-chain — council F3, follow-on);
    /// 3. only then does the rail-agnostic [`PaymentProvider`] move the money, under a
    ///    signature-derived idempotency key. The outcome is handled two-generals-honestly (S29):
    ///    provably-NOT-charged refunds the reservation; INDETERMINATE (timeout/5xx/panic — the
    ///    charge may have posted) KEEPS it, because refunding against money that may have moved
    ///    would let real spend exceed the cap;
    /// 4. on success the receipt reports `execute` of `<amount>` to `<payee>` — the reconciliation
    ///    Matches iff the executor charged exactly the declared amount (a non-canonical amount is
    ///    declined up front, F4).
    ///
    /// BOUNDS (council): the cap is PER-CAPSULE, not per-mandate (the right "cap this agent" bound;
    /// per-mandate caps are a follow-on — key the meter on `grant_id`). The [`SpendMeter`] has two
    /// modes (S28): serve() wires a DURABLE one (`open_durable` — the reservation persists BEFORE
    /// money moves; a restart never refills the cap) and the operator provisioning surface
    /// (`POST /api/spend-budgets`) refuses to put a money cap on a non-durable meter; a bare
    /// in-memory meter remains available for tests/embedded and rate-limiting uses. A crash between
    /// the persisted reservation and the rail leaves an ORPHANED reservation: the intent id is
    /// burned (no replay) and no money moved, so the cap honestly over-counts — fail-closed; the
    /// operator's recovery lever is raising the limit — AFTER reconciling with the rail: an
    /// indeterminate reservation may correspond to a charge that DID post, so a blind cap raise
    /// can authorize real spend beyond the original intent (council S29 red-team F4).
    /// The REAL rail is [`HttpPaymentProvider`] (`ELASTOS_PAYMENT_ENDPOINT`, https-enforced,
    /// durable meter required); the Mock stays dev/demo-gated (`ELASTOS_ALLOW_MOCK_PAYMENTS`).
    /// `ledger` records every rail attempt (Sprint 30): performed (with the rail reference),
    /// provably-not-charged, and PENDING for indeterminate outcomes — the operator's
    /// reconciliation work list. The ledger never gates money (its failures are reported in the
    /// reason, not enforced); pass `PaymentLedger::new()` where reconciliation isn't exercised.
    /// Register `runtime.market_quote` (Sprint 39) — the READ affordance that lets an agent shop
    /// within its mandate: quote the live terms of exactly the asset its pay-mandate scopes.
    ///
    /// - MANDATE-SCOPED, no market-wide oracle: the resource is the SAME pay resource
    ///   (`elastos://runtime/pay/<asset>`) the buy uses, so the envelope gate confines quoting to
    ///   the assets the operator granted — an agent can never price-scan the marketplace for free.
    /// - READ-ONLY through the ONE quote spine (`crate::market_quote`): the same single-flight,
    ///   TTL-cached path the Marketplace panel reads — one live chain read per asset per window,
    ///   whoever asks. No keys, no broadcast (P3).
    /// - HONEST reconciliation, two modes on the declared `input_hash`:
    ///   * `""` (discovery): the read performs as declared (`Matched` ⇒ `performed`) and the
    ///     terms reach the agent via the response's explicit-disclosure channel.
    ///   * the canonical terms string (attested): the executor echoes the ACTUAL terms, so
    ///     `Matched` PROVES "the terms are what I believed" and a changed listing reconciles
    ///     `Diverged` — never a fabricated match.
    ///
    ///   A failed read (no listing, chain unreachable, sold out) DECLINES with the bounded error
    ///   (⇒ `authorized_not_performed`) — a quote is `performed` only when it truly returned
    ///   terms.
    /// - The terms are ephemeral agent data: they ride the response, not the signed chain (the
    ///   receipt records the quote ACT; no price data lands on-chain beyond what it already
    ///   carries).
    ///
    /// HONEST BOUNDS: an attested `Matched` proves the terms as of the spine's LAST READ (at most
    /// `MARKET_QUOTE_TTL_SECS` old) — a listing changed inside the cache window is caught on the
    /// next re-read, not instantly. In the dev/chain-mock rights modes `quote_buy` returns a
    /// synthetic FREE quote (as the Marketplace panel also states) — the disclosure carries those
    /// synthetic terms; only the live Chain mode reads real listings. Quote dispatches consume
    /// the per-mandate dispatch budget like any act.
    pub fn with_market_quotes(
        mut self,
        cache: crate::market_quote::MarketQuoteCache,
        quoter: Arc<dyn crate::market_quote::MarketQuoter>,
    ) -> Self {
        // Bound on CONCURRENT in-flight quote reads (council S39 red-team F1 — the same wedge
        // class as MAX_INFLIGHT_PAYMENTS): the chain read blocks its dispatch thread for up to
        // a few chain conversations x the S40 read deadline, and single-flight only dedups the
        // SAME asset — K distinct
        // assets against a slow RPC would otherwise park K blocking threads at once and starve
        // the pay pipeline's own pool. Over the bound: refuse with retry, never queue.
        const MAX_INFLIGHT_QUOTES: usize = 8;
        let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.register(
            "runtime.market_quote",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // Scoped to the pay namespace — the asset is the suffix, exactly as for the buy.
                let Some(asset) = intent.resource.strip_prefix(PAY_PREFIX) else {
                    return IntentExecution::Declined {
                        reason: format!("market_quote resource must be {PAY_PREFIX}<asset>"),
                    };
                };
                if !valid_slug_1_64(asset) {
                    return IntentExecution::Declined {
                        reason: "market_quote asset must be 1-64 chars of [A-Za-z0-9._-]"
                            .to_string(),
                    };
                }
                // The declared input_hash is either empty (discovery) or the expected canonical
                // terms (attested). Bounded before any read: it lands on the signed chain.
                let declared = intent.input_hash.trim();
                if declared.len() > 160 || !declared.chars().all(|c| c.is_ascii_graphic()) {
                    return IntentExecution::Declined {
                        reason: "market_quote expected-terms (input_hash) must be <=160 \
                                 printable ASCII chars (or empty for discovery)"
                            .to_string(),
                    };
                }
                // Claim an in-flight slot; the RAII guard releases it on EVERY exit path.
                struct QuoteSlot(Arc<std::sync::atomic::AtomicUsize>);
                impl Drop for QuoteSlot {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                let prior = in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _slot = QuoteSlot(in_flight.clone());
                if prior >= MAX_INFLIGHT_QUOTES {
                    return IntentExecution::Declined {
                        reason: format!(
                            "market_quote refused: {MAX_INFLIGHT_QUOTES} quote reads already \
                             in-flight (fail-closed concurrency bound; retry shortly)"
                        ),
                    };
                }
                match crate::market_quote::quote_single_flight(
                    &cache,
                    quoter.as_ref(),
                    asset,
                    crate::market_quote::now_unix(),
                ) {
                    Ok(quote) => match quote.canonical_terms() {
                        // The echo lands on the SIGNED chain (attested mode) and in the agent
                        // response: hold chain-sourced terms to the SAME bound as the agent's
                        // declaration (council S39 red-team F2 — validate what we sign, not
                        // only what we receive). Structurally unreachable via the real decode
                        // (u128 decimal + fixed hex), so out-of-bound terms mean a broken or
                        // hostile quote source ⇒ refuse.
                        Some(terms)
                            if terms.len() > 160
                                || !terms.chars().all(|c| c.is_ascii_graphic()) =>
                        {
                            IntentExecution::Declined {
                                reason: "market_quote refused: the quote source returned \
                                         malformed terms (out of bound)"
                                    .to_string(),
                            }
                        }
                        Some(terms) => IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // Discovery ("" declared): echo the declaration — the READ performed
                            // exactly as declared, so it reconciles Matched and the terms travel
                            // via the disclosure channel. Attested (terms declared): echo the
                            // ACTUAL terms — Matched proves the belief; a changed listing
                            // Diverges, never a fabricated match.
                            input_hash: if declared.is_empty() {
                                String::new()
                            } else {
                                terms.clone()
                            },
                            resource: intent.resource.clone(),
                            action: "read".to_string(),
                            rail_ref: None,
                            // The EXPLICIT disclosure: a quote's whole point is that the agent
                            // learns the terms (public listing data, not a secret — unlike
                            // state_get's values, which stay one-bit).
                            agent_visible_report: Some(terms),
                        },
                        // The read returned an error outcome (no listing / sold out / chain
                        // unreachable) ⇒ authorized_not_performed, honestly reasoned.
                        None => IntentExecution::Declined {
                            reason: format!(
                                "market_quote could not read the listing: {}",
                                quote.error.as_deref().unwrap_or("no terms returned")
                            ),
                        },
                    },
                    // Another consumer's read for this asset is in flight — refuse to duplicate
                    // it (the single-flight bound); the agent retries shortly.
                    Err(crate::market_quote::ReadInFlight) => IntentExecution::Declined {
                        reason: "market_quote in progress for this asset — retry shortly"
                            .to_string(),
                    },
                }
            }),
        );
        self
    }

    pub fn with_payments(
        mut self,
        meter: Arc<elastos_runtime::primitives::spend::SpendMeter>,
        provider: Arc<dyn PaymentProvider>,
        ledger: Arc<crate::payment_ledger::PaymentLedger>,
    ) -> Self {
        // Bound on CONCURRENT in-flight payments (council S29 red-team F2): the rail call blocks
        // the dispatching thread for up to its timeout, and the per-mandate rate budget bounds
        // acts-per-window, not concurrency — without this cap a slow/hostile rail × parallel
        // dispatches could wedge every async worker. Fail-closed: over the bound, the payment is
        // REFUSED before any reservation (nothing debited, nothing sent), never queued.
        const MAX_INFLIGHT_PAYMENTS: usize = 8;
        let in_flight = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        self.register(
            "runtime.pay",
            Arc::new(move |intent: &IntentDeclarationV1| {
                // Scoped to a PAYEE; the payee is the suffix. Outside the namespace ⇒ Decline.
                let Some(payee) = intent.resource.strip_prefix(PAY_PREFIX) else {
                    return IntentExecution::Declined {
                        reason: format!("pay resource must be {PAY_PREFIX}<payee>"),
                    };
                };
                if !valid_slug_1_64(payee) {
                    return IntentExecution::Declined {
                        reason: "pay payee must be 1-64 chars of [A-Za-z0-9._-]".to_string(),
                    };
                }
                // The AMOUNT rides in the signed `input_hash` as a decimal integer of spend units.
                // A non-integer or zero amount is malformed ⇒ Decline (a payment must be positive).
                let amount: u64 = match intent.input_hash.parse() {
                    Ok(n) if n > 0 => n,
                    _ => {
                        return IntentExecution::Declined {
                            reason: "pay amount (input_hash) must be a positive integer of spend units"
                                .to_string(),
                        };
                    }
                };
                // Canonical form (council F4): a Performed pay MUST reconcile Matched, so the declared
                // amount must equal the canonical decimal of what we charge. Reject "0200"/"+200"/
                // " 200" — they parse to 200 but the receipt would echo "200" and Diverge, recording
                // real money moved under a success=false use. A non-canonical amount pays nothing.
                if amount.to_string() != intent.input_hash {
                    return IntentExecution::Declined {
                        reason: "pay amount must be a canonical decimal (no leading zero, sign, or space)"
                            .to_string(),
                    };
                }
                // Concurrency gate BEFORE the reservation: an over-bound payment is refused with
                // nothing debited and nothing sent. The RAII guard releases the slot on EVERY
                // exit path below, including a provider panic (caught by catch_unwind).
                struct InFlightSlot(Arc<std::sync::atomic::AtomicUsize>);
                impl Drop for InFlightSlot {
                    fn drop(&mut self) {
                        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                    }
                }
                // Claim an in-flight slot; the RAII guard releases it on EVERY exit path
                // (including the over-bound refusal below and a rail panic).
                let prior = in_flight.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _slot = InFlightSlot(in_flight.clone());
                if prior >= MAX_INFLIGHT_PAYMENTS {
                    return IntentExecution::Declined {
                        reason: format!(
                            "payment refused: {MAX_INFLIGHT_PAYMENTS} payments already \
                             in-flight (fail-closed concurrency bound; retry later)"
                        ),
                    };
                }
                // The idempotency key: unique per SIGNED declaration (an intent_id can recycle
                // once it ages out of the replay window; a signature cannot).
                let idempotency_key = format!("flint-{}", intent.signature);
                // DURABLE ON-RAIL IDEMPOTENCY (Sprint 35, closes the cross-window double-buy):
                // the replay guard blocks a re-dispatched identical signed intent only WITHIN its
                // window; once it ages out, the same signed intent would reach here again and,
                // absent this check, buy/charge a SECOND time. The durable ledger is the dedup: if
                // this key already carries a money-moved-or-may-have entry (Performed, Pending, or
                // ResolvedCharged), the payment already happened — refuse fail-closed WITHOUT
                // reserving or paying again (idempotent no-op). A prior NotCharged/ResolvedNotCharged
                // (provably nothing moved) is allowed to retry. This read is a FAST PATH: the same
                // invariant is enforced atomically by `begin_attempt` → `AlreadyActive` below; the
                // pre-check just refuses before the durable debit+refund round-trip.
                // RESIDUAL (MKT-DRM): the ledger
                // eviction cap means a very old terminal key can be evicted, reopening a retry —
                // bounded, and pending/charged keys are never evicted.
                if let Some(existing) = ledger.get(&idempotency_key) {
                    use crate::payment_ledger::PaymentStatus::*;
                    if matches!(existing.status, Performed | Pending | ResolvedCharged) {
                        return IntentExecution::Declined {
                            reason: format!(
                                "payment already settled or pending under idempotency key \
                                 {idempotency_key} (status {:?}) — not re-charged (idempotent); a \
                                 re-dispatch of a signed intent never moves money twice",
                                existing.status
                            ),
                        };
                    }
                }
                // RESERVE against the cap FIRST — atomic, fail-closed. Over budget (or an
                // unprovisioned capsule ⇒ zero) refuses here; no money can move.
                if let Err(e) = meter.try_debit(&intent.capsule, amount) {
                    return IntentExecution::Declined {
                        reason: format!("payment refused by spend cap: {e}"),
                    };
                }
                // DURABLE CUSTODY BEFORE THE BROADCAST (council S35 red-team F1): record the
                // idempotency key as Pending on the ledger BEFORE moving any money, so a
                // re-dispatch can never find "no entry" for a buy whose funds moved. If the ledger
                // cannot custody the attempt (per-capsule pending cap, ledger full of money-bearing
                // keys, persist failure), REFUND and DECLINE without ever broadcasting — money
                // never moves into an unrecordable state. A concurrent dispatch that already began
                // this key is refused idempotently (its reservation is the live one; this one's is
                // refunded).
                use crate::payment_ledger::{BeginAttempt, PaymentStatus};
                match ledger.begin_attempt_on_rail(
                    &idempotency_key,
                    &intent.capsule,
                    payee,
                    amount,
                    Some(intent.standing_grant_id.as_str()),
                    // Stamp the paying rail so the DRM reconciler selects its pendings by tag, not
                    // by the rail-controlled note (Sprint 44 / MKT-DRM 2d).
                    provider.rail(),
                ) {
                    BeginAttempt::Started => {}
                    BeginAttempt::AlreadyActive(status) => {
                        let _ = meter.try_refund(&intent.capsule, amount);
                        return IntentExecution::Declined {
                            reason: format!(
                                "payment already in flight or settled under idempotency key \
                                 {idempotency_key} (status {status:?}) — not moved again \
                                 (idempotent); this attempt's reservation was released"
                            ),
                        };
                    }
                    BeginAttempt::CapacityRefused => {
                        let _ = meter.try_refund(&intent.capsule, amount);
                        return IntentExecution::Declined {
                            reason: format!(
                                "payment refused: the ledger cannot durably custody this attempt \
                                 (key {idempotency_key}) — refusing to move money into an \
                                 unrecordable state (fail-closed); retry after pending \
                                 reconciliation frees capacity"
                            ),
                        };
                    }
                }
                // The cap allowed it AND the attempt is durably custodied — NOW move the money on
                // the rail, then FINALIZE the Pending placeholder to the outcome.
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    provider.pay(payee, amount, &idempotency_key)
                }));
                match outcome {
                    Ok(Ok(rail_ref)) => {
                        tracing::info!(
                            capsule = %intent.capsule,
                            amount,
                            idempotency_key = %idempotency_key,
                            rail_ref = %rail_ref.chars().take(128).collect::<String>(),
                            "payment performed on the rail"
                        );
                        // Sanitize the rail-controlled reference to the same printable/bounded
                        // discipline BEFORE it enters a signed audit field — a rail (or a DRM chain
                        // adapter) must never inject control bytes into the receipt.
                        let rail_ref = crate::payment_ledger::sanitize_rail_note(&rail_ref);
                        ledger.finalize(&idempotency_key, PaymentStatus::Performed, &rail_ref);
                        IntentExecution::Performed {
                            capsule: intent.capsule.clone(),
                            method_id: intent.method_id.clone(),
                            // Echo the amount actually charged (== declared) so reconcile Matches;
                            // the receipt's CapabilityUse names a payment of this amount+payee.
                            input_hash: amount.to_string(),
                            resource: intent.resource.clone(),
                            action: "execute".to_string(),
                            rail_ref: (!rail_ref.is_empty()).then_some(rail_ref),
                            agent_visible_report: None,
                        }
                    }
                    Ok(Err(PayError::NotCharged(rail_err))) => {
                        // PROVABLY not charged — finalize NotCharged and REFUND. The signed reason
                        // claims "refunded" ONLY when the refund is durably in force (council S28
                        // F3): a durable refund that cannot persist is rolled back by try_refund,
                        // and the honest record is that the cap remains debited (fail-closed).
                        ledger.finalize(&idempotency_key, PaymentStatus::NotCharged, &rail_err);
                        IntentExecution::Declined {
                            reason: match meter.try_refund(&intent.capsule, amount) {
                                Ok(_) => format!("payment rail refused (spend refunded): {rail_err}"),
                                Err(e) => format!(
                                    "payment rail refused and the refund could not be durably \
                                     recorded ({e}) — the cap remains debited: {rail_err}"
                                ),
                            },
                        }
                    }
                    Ok(Err(PayError::Indeterminate(rail_err))) => {
                        // The outcome is UNKNOWN — the charge may have posted. Refunding here would
                        // let real spend exceed the cap (charge landed + headroom restored), the
                        // one unbreakable invariant — so the reservation is KEPT, the entry stays
                        // Pending with the rail reference in its note, resolvable via the
                        // idempotency key. Fail-closed over-counting.
                        ledger.finalize(&idempotency_key, PaymentStatus::Pending, &rail_err);
                        IntentExecution::Declined {
                            reason: format!(
                                "payment outcome INDETERMINATE ({rail_err}) — not attested as \
                                 performed; the reservation is KEPT (cap remains debited) pending \
                                 rail reconciliation under idempotency key {idempotency_key}"
                            ),
                        }
                    }
                    Err(_panic) => {
                        // The rail PANICKED mid-call. With a REAL rail refunding is UNSAFE — the
                        // panic may have happened AFTER the charge posted. INDETERMINATE: keep the
                        // reservation, the entry stays Pending.
                        ledger.finalize(&idempotency_key, PaymentStatus::Pending, "rail panicked");
                        IntentExecution::Declined {
                            reason: format!(
                                "payment rail panicked — outcome INDETERMINATE; the reservation is \
                                 KEPT (cap remains debited) pending rail reconciliation under \
                                 idempotency key {idempotency_key}"
                            ),
                        }
                    }
                }
            }),
        );
        self
    }

    pub fn register(&mut self, method_id: &str, executor: MethodFn) {
        self.methods.insert(method_id.to_string(), executor);
    }
}

impl IntentExecutor for MethodRegistryExecutor {
    fn execute(&self, intent: &IntentDeclarationV1) -> IntentExecution {
        match self.methods.get(&intent.method_id) {
            Some(executor) => executor(intent),
            None => IntentExecution::Declined {
                reason: format!("no executor registered for method {}", intent.method_id),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(method: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "i1",
            "vm-agent",
            method,
            "cafe",
            "elastos://pay/vendor",
            "write",
            "grant-1",
        )
    }

    #[test]
    fn registered_method_performs_and_reports_its_fields() {
        let mut reg = MethodRegistryExecutor::new();
        reg.register(
            "demo.read",
            Arc::new(|i: &IntentDeclarationV1| IntentExecution::Performed {
                capsule: i.capsule.clone(),
                method_id: i.method_id.clone(),
                input_hash: i.input_hash.clone(),
                resource: i.resource.clone(),
                action: i.action.clone(),
                rail_ref: None,
                agent_visible_report: None,
            }),
        );
        match reg.execute(&intent("demo.read")) {
            IntentExecution::Performed {
                method_id,
                resource,
                ..
            } => {
                assert_eq!(method_id, "demo.read");
                assert_eq!(resource, "elastos://pay/vendor");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    #[test]
    fn production_declines_unwired_methods_and_performs_the_real_audit_read() {
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(AuditLog::with_file(dir.path().join("audit.log")).unwrap());
        log.emit(
            elastos_runtime::primitives::audit::AuditEvent::RuntimeStart {
                timestamp: elastos_common::SecureTimestamp::now(),
                version: "t".to_string(),
            },
        )
        .unwrap();
        let reg = MethodRegistryExecutor::production(log, None);
        // Unwired methods decline (⇒ Undelivered), never a fabricated match.
        assert!(matches!(
            reg.execute(&intent("pay.invoke")),
            IntentExecution::Declined { .. }
        ));
        // The real affordance PERFORMS against a signed, verifiable chain and reports action=read.
        match reg.execute(&intent("runtime.audit_verify")) {
            IntentExecution::Performed { action, .. } => assert_eq!(action, "read"),
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    #[test]
    fn audit_verify_declines_on_a_memory_only_chain() {
        // Real state drives the outcome: a memory-only log has nothing durable to verify ⇒ Declined,
        // never a fabricated "performed".
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        assert!(matches!(
            reg.execute(&intent("runtime.audit_verify")),
            IntentExecution::Declined { .. }
        ));
    }

    fn notify_intent(resource: &str, capsule: &str, args: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "notify-1",
            capsule,
            "runtime.notify",
            args,
            resource,
            "message",
            "grant-1",
        )
    }

    /// The first side-effecting affordance: `runtime.notify` PERFORMS iff the notification
    /// actually LANDS in the operator's Inbox store — and the delivered row is real, readable
    /// state (visible to the Inbox app via `load_summary`), not a claim.
    #[test]
    fn notify_delivers_a_real_inbox_notification_and_reports_message() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        match reg.execute(&notify_intent(&resource, "vm-agent", "cafe")) {
            IntentExecution::Performed {
                action,
                resource: r,
                input_hash,
                ..
            } => {
                assert_eq!(action, "message", "the act performed IS a message");
                assert_eq!(r, resource, "delivered to the declared topic");
                assert_eq!(input_hash, "cafe", "the consumed input hash is reported");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        // The side effect is REAL: the Inbox summary shows the delivered row.
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(summary.unread_count, 1, "one unread notification landed");
        let entry = &summary.entries[0];
        assert_eq!(entry.kind, crate::notifications::AGENT_ACT_KIND);
        assert!(
            entry.title.contains("agent-status"),
            "title names the topic"
        );
        assert!(
            entry.body.contains("vm-agent"),
            "body names the acting capsule"
        );
        assert!(entry.body.contains("grant-1"), "body names the mandate");
        assert!(
            entry.body.contains("cafe"),
            "body carries the input-hash commitment"
        );
    }

    /// Fail-closed scoping: outside the inbox namespace, or with a topic that could smuggle
    /// content into the operator surface, notify DECLINES — and nothing lands in the store.
    #[test]
    fn notify_declines_bad_scopes_and_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for bad in [
            "elastos://mail/send".to_string(), // outside the namespace
            INBOX_NOTIFY_PREFIX.to_string(),   // empty topic
            format!("{INBOX_NOTIFY_PREFIX}<script>x</script>"), // markup smuggle
            format!("{INBOX_NOTIFY_PREFIX}a/b"), // path trick
            format!("{INBOX_NOTIFY_PREFIX}{}", "x".repeat(65)), // over-long
        ] {
            assert!(
                matches!(
                    reg.execute(&notify_intent(&bad, "vm-agent", "")),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {bad:?}"
            );
        }
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert_eq!(
            summary.entries.len(),
            0,
            "a declined notify delivers NOTHING"
        );
    }

    /// Council F1: `intent_id` and `input_hash` reach the operator's Inbox body, so a malformed
    /// one (free text an agent could use to phish the operator, or a giant string to bloat the
    /// row) DECLINES — nothing is delivered. A clean slug intent_id + hex input_hash still deliver.
    #[test]
    fn notify_declines_operator_unsafe_intent_fields_and_delivers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        let signed = |intent_id: &str, input_hash: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                intent_id,
                "vm-agent",
                "runtime.notify",
                input_hash,
                &resource,
                "message",
                "grant-1",
            )
        };
        // A phishing intent_id (spaces, punctuation) — declined.
        assert!(matches!(
            reg.execute(&signed("URGENT: run revoke-all now", "")),
            IntentExecution::Declined { .. }
        ));
        // A non-hex input_hash reaching the body — declined.
        assert!(matches!(
            reg.execute(&signed("intent-1", "drain the vault")),
            IntentExecution::Declined { .. }
        ));
        // An over-long intent_id (row-bloat) — declined.
        assert!(matches!(
            reg.execute(&signed(&"a".repeat(65), "")),
            IntentExecution::Declined { .. }
        ));
        assert_eq!(
            crate::notifications::load_summary(dir.path())
                .unwrap()
                .entries
                .len(),
            0,
            "no operator-unsafe field ever delivered a row"
        );
        // A clean slug id + hex input_hash still delivers.
        assert!(matches!(
            reg.execute(&signed("intent-abc_1.2", "cafe01")),
            IntentExecution::Performed { .. }
        ));
    }

    /// Council F1 (flood): agent-act rows are hard-capped, so an agent flooding distinct intents
    /// under ONE mandate cannot grow the operator's Inbox store without bound.
    #[test]
    fn notify_flood_is_bounded_by_the_agent_act_cap() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        for i in 0..400u32 {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            let intent = IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                &format!("intent-{i}"),
                "vm-agent",
                "runtime.notify",
                "",
                &resource,
                "message",
                "grant-1",
            );
            assert!(matches!(
                reg.execute(&intent),
                IntentExecution::Performed { .. }
            ));
        }
        let summary = crate::notifications::load_summary(dir.path()).unwrap();
        assert!(
            summary.entries.len() <= 256,
            "agent-act rows are capped at 256, got {}",
            summary.entries.len()
        );
    }

    fn state_put_intent(resource: &str, capsule: &str, value_hash: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "state-intent-1",
            capsule,
            "runtime.state_put",
            value_hash,
            resource,
            "write",
            "grant-1",
        )
    }

    /// The SECOND side-effecting affordance: `runtime.state_put` PERFORMS iff the durable write
    /// LANDS, and the written value is readable back — a real, observable mutation, principal-scoped.
    #[test]
    fn state_put_writes_durable_readable_state_and_reports_write() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        match reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")) {
            IntentExecution::Performed {
                action,
                resource: r,
                input_hash,
                ..
            } => {
                assert_eq!(action, "write", "the act performed IS a write");
                assert_eq!(r, resource);
                assert_eq!(input_hash, "cafe01");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        // The side effect is REAL and readable back — principal-scoped to the acting capsule.
        let got = crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
            .unwrap()
            .expect("the written key is readable back");
        assert_eq!(got.value_hash, "cafe01");
        assert_eq!(got.grant_id, "grant-1");
        // A DIFFERENT capsule cannot read it — no cross-principal state leak.
        assert!(
            crate::agent_store::get_agent_state(dir.path(), "vm-other", "cursor")
                .unwrap()
                .is_none()
        );
    }

    /// Fail-closed scoping: outside the store namespace, or with a key/value that could smuggle
    /// free text into durable state, state_put DECLINES — and nothing is persisted.
    #[test]
    fn state_put_declines_bad_scopes_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for (resource, value) in [
            ("elastos://mail/send".to_string(), "aa".to_string()), // outside namespace
            (STATE_PUT_PREFIX.to_string(), "aa".to_string()),      // empty key
            (format!("{STATE_PUT_PREFIX}a/b"), "aa".to_string()),  // path trick
            (format!("{STATE_PUT_PREFIX}k"), "not hex".to_string()), // free-text value
            (
                format!("{STATE_PUT_PREFIX}{}", "x".repeat(65)),
                "aa".to_string(),
            ), // over-long key
        ] {
            assert!(
                matches!(
                    reg.execute(&state_put_intent(&resource, "vm-agent", &value)),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {resource:?} value {value:?}"
            );
        }
        assert!(
            crate::agent_store::get_agent_state(dir.path(), "vm-agent", "k")
                .unwrap()
                .is_none(),
            "a declined state_put persists NOTHING"
        );
    }

    /// The PERSISTED intent_id is bounded like every other agent-chosen stored string (council
    /// carry-over): a giant/free-form intent_id declines rather than bloating durable state.
    #[test]
    fn state_put_declines_an_unbounded_intent_id() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let intent = IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            &"z".repeat(65), // over-long intent_id
            "vm-agent",
            "runtime.state_put",
            "cafe01",
            &resource,
            "write",
            "grant-1",
        );
        assert!(matches!(
            reg.execute(&intent),
            IntentExecution::Declined { .. }
        ));
        assert!(
            crate::agent_store::get_agent_state(dir.path(), "vm-agent", "cursor")
                .unwrap()
                .is_none(),
            "a declined write persists nothing"
        );
    }

    /// Without a data dir there is no store to write into — state_put is honestly UNWIRED.
    #[test]
    fn state_put_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        assert!(matches!(
            reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    fn state_get_intent(resource: &str, capsule: &str, expected: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "state-get-1",
            capsule,
            "runtime.state_get",
            expected, // the value the agent EXPECTS (input_hash)
            resource,
            "read",
            "grant-1",
        )
    }

    /// Sprint 25: `runtime.state_get` is the READ side of the KV. It echoes the ACTUAL stored
    /// value-hash (so the read reconciles Matched only when the agent declared the right value — an
    /// attested "K = V" — proven end-to-end in the handler tests), Declines an absent key, and is
    /// PRINCIPAL-SCOPED (an agent reads only its own state).
    #[test]
    fn state_get_reads_back_own_state_attested_and_principal_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        // Seed the store via the real state_put affordance.
        assert!(matches!(
            reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Performed { .. }
        ));

        // A read → Performed echoing the ACTUAL stored value, action "read" — regardless of what
        // the agent declared, so reconcile can Match (declared==actual) or Diverge (declared!=actual).
        for declared in ["cafe01", "beef99", ""] {
            match reg.execute(&state_get_intent(&resource, "vm-agent", declared)) {
                IntentExecution::Performed {
                    action,
                    resource: r,
                    input_hash,
                    ..
                } => {
                    assert_eq!(action, "read", "the act performed IS a read");
                    assert_eq!(r, resource);
                    assert_eq!(
                        input_hash, "cafe01",
                        "echoes the REAL stored value-hash, not the agent's claim ({declared:?})"
                    );
                }
                other => panic!("expected Performed for declared {declared:?}, got {other:?}"),
            }
        }

        // A DIFFERENT principal reading the same key → Declined (no cross-principal state read).
        assert!(
            matches!(
                reg.execute(&state_get_intent(&resource, "vm-other", "cafe01")),
                IntentExecution::Declined { .. }
            ),
            "an agent can only read its OWN state — never another principal's"
        );

        // An ABSENT key for the acting principal → Declined (authorized_not_performed).
        let absent = format!("{STATE_PUT_PREFIX}never-written");
        assert!(matches!(
            reg.execute(&state_get_intent(&absent, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    /// Fail-closed scoping for the read: outside the store namespace, a bad key, or an unbounded
    /// expected-value DECLINES — the read affordance is as strict about its inputs as the write.
    #[test]
    fn state_get_declines_bad_scopes() {
        let dir = tempfile::tempdir().unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        for (resource, expected) in [
            ("elastos://mail/send".to_string(), "aa".to_string()), // outside namespace
            (STATE_PUT_PREFIX.to_string(), "aa".to_string()),      // empty key
            (format!("{STATE_PUT_PREFIX}a/b"), "aa".to_string()),  // path trick
            (format!("{STATE_PUT_PREFIX}k"), "not hex".to_string()), // free-text expected value
        ] {
            assert!(
                matches!(
                    reg.execute(&state_get_intent(&resource, "vm-agent", &expected)),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {resource:?} expected {expected:?}"
            );
        }
    }

    /// Without a data dir there is no store to read — state_get is honestly UNWIRED.
    #[test]
    fn state_get_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        assert!(matches!(
            reg.execute(&state_get_intent(&resource, "vm-agent", "cafe01")),
            IntentExecution::Declined { .. }
        ));
    }

    // ── Sprint 27: the spend-capped `runtime.pay` affordance ──────────────────────────────────
    use elastos_runtime::primitives::spend::SpendMeter;

    /// A payment rail that always refuses PROVABLY-NOT-CHARGED — to prove the meter is REFUNDED
    /// exactly (and only) on that classification.
    struct FailingProvider;
    impl PaymentProvider for FailingProvider {
        fn rail(&self) -> crate::payment_ledger::PaymentRail {
            crate::payment_ledger::PaymentRail::Unknown
        }
        fn pay(&self, _payee: &str, _amount: u64, _key: &str) -> Result<String, PayError> {
            Err(PayError::NotCharged("rail unavailable".to_string()))
        }
    }

    fn pay_intent(payee: &str, capsule: &str, amount: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "pay-intent-1",
            capsule,
            "runtime.pay",
            amount, // the AMOUNT rides in input_hash
            &format!("{PAY_PREFIX}{payee}"),
            "execute",
            "grant-1",
        )
    }

    fn pay_registry(meter: Arc<SpendMeter>) -> (MethodRegistryExecutor, Arc<MockPaymentProvider>) {
        let provider = Arc::new(MockPaymentProvider::default());
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter,
                provider.clone(),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        (reg, provider)
    }

    /// `runtime.pay` within the cap: the meter is debited and the rail moves the money once — the
    /// receipt reports the exact amount + payee (Performed as `execute`).
    #[test]
    fn pay_within_cap_charges_the_meter_and_moves_money_once() {
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 500).unwrap();
        let (reg, provider) = pay_registry(meter.clone());
        match reg.execute(&pay_intent("acme-vendor", "vm-agent", "200")) {
            IntentExecution::Performed {
                action,
                resource,
                input_hash,
                ..
            } => {
                assert_eq!(action, "execute");
                assert_eq!(resource, format!("{PAY_PREFIX}acme-vendor"));
                assert_eq!(
                    input_hash, "200",
                    "the receipt names the amount actually paid"
                );
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        assert_eq!(
            meter.remaining("vm-agent"),
            300,
            "the cap was debited by exactly the amount"
        );
        assert_eq!(
            *provider.payments.lock().unwrap(),
            vec![("acme-vendor".to_string(), 200)],
            "the rail moved the money exactly once"
        );
    }

    /// Over the cap: the payment is REFUSED, the meter is untouched, and NO money moves — a
    /// fail-closed signed refusal, the whole point of the affordance.
    #[test]
    fn pay_over_cap_is_refused_and_moves_no_money() {
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 100).unwrap();
        let (reg, provider) = pay_registry(meter.clone());
        assert!(matches!(
            reg.execute(&pay_intent("acme-vendor", "vm-agent", "150")),
            IntentExecution::Declined { .. }
        ));
        assert_eq!(
            meter.remaining("vm-agent"),
            100,
            "a refused payment does not touch the cap"
        );
        assert!(
            provider.payments.lock().unwrap().is_empty(),
            "no money moved over the cap"
        );
    }

    /// An unprovisioned capsule has ZERO budget (fail-closed) — it cannot pay a cent until the
    /// operator provisions a cap.
    #[test]
    fn pay_unprovisioned_capsule_is_fail_closed() {
        let meter = Arc::new(SpendMeter::new());
        let (reg, provider) = pay_registry(meter);
        assert!(matches!(
            reg.execute(&pay_intent("acme-vendor", "vm-nobudget", "1")),
            IntentExecution::Declined { .. }
        ));
        assert!(provider.payments.lock().unwrap().is_empty());
    }

    /// If the RAIL fails after the reservation, the meter is REFUNDED (no money moved) so the budget
    /// is made whole — a later in-budget payment still works.
    #[test]
    fn pay_rail_failure_refunds_the_reservation() {
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 100).unwrap();
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter.clone(),
                Arc::new(FailingProvider),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        assert!(matches!(
            reg.execute(&pay_intent("acme-vendor", "vm-agent", "40")),
            IntentExecution::Declined { .. }
        ));
        assert_eq!(
            meter.remaining("vm-agent"),
            100,
            "a failed rail refunds the reservation — the cap is made whole, no phantom spend"
        );
    }

    /// A rail that PANICS is INDETERMINATE (S29 — supersedes the S27 refund-on-panic fold): with a
    /// REAL rail the panic may happen AFTER the charge posted, and a refund would let total real
    /// spend exceed the cap (invariant a). The reservation is KEPT and the reason says so honestly.
    #[test]
    fn pay_rail_panic_keeps_the_reservation_as_indeterminate() {
        struct PanickingProvider;
        impl PaymentProvider for PanickingProvider {
            fn rail(&self) -> crate::payment_ledger::PaymentRail {
                crate::payment_ledger::PaymentRail::Unknown
            }
            fn pay(&self, _payee: &str, _amount: u64, _key: &str) -> Result<String, PayError> {
                panic!("rail exploded mid-charge")
            }
        }
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 100).unwrap();
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter.clone(),
                Arc::new(PanickingProvider),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        match reg.execute(&pay_intent("acme-vendor", "vm-agent", "40")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("INDETERMINATE") && reason.contains("cap remains debited"),
                    "the signed reason must state indeterminacy + kept reservation: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
        assert_eq!(
            meter.remaining("vm-agent"),
            60,
            "the reservation is KEPT — a maybe-charged payment must not restore headroom"
        );
    }

    /// An INDETERMINATE rail outcome (timeout/5xx) keeps the reservation and names the idempotency
    /// key for reconciliation — refunding against money that may have moved would break the cap.
    #[test]
    fn pay_indeterminate_outcome_keeps_the_reservation() {
        struct IndeterminateProvider;
        impl PaymentProvider for IndeterminateProvider {
            fn rail(&self) -> crate::payment_ledger::PaymentRail {
                crate::payment_ledger::PaymentRail::Unknown
            }
            fn pay(&self, _payee: &str, _amount: u64, _key: &str) -> Result<String, PayError> {
                Err(PayError::Indeterminate("timeout after send".to_string()))
            }
        }
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 100).unwrap();
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter.clone(),
                Arc::new(IndeterminateProvider),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        match reg.execute(&pay_intent("acme-vendor", "vm-agent", "40")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("INDETERMINATE") && reason.contains("idempotency key flint-"),
                    "the reason names the indeterminacy and the reconciliation key: {reason}"
                );
                assert!(
                    !reason.contains("refunded"),
                    "an indeterminate outcome must never claim a refund: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
        assert_eq!(meter.remaining("vm-agent"), 60, "the reservation is kept");
    }

    /// The HTTP rail connector's two-generals classification, exercised against a REAL local HTTP
    /// server: 2xx confirms, 4xx is provably-not-charged, 5xx is indeterminate, connection-refused
    /// (nothing sent) is provably-not-charged, and a timeout after send is indeterminate. The
    /// idempotency key must reach the wire as the Idempotency-Key header.
    #[test]
    fn http_rail_classifies_outcomes_two_generals_honestly() {
        use std::io::{Read as _, Write as _};
        // A one-shot local HTTP server: returns `status`, captures the request head.
        fn serve_once(status: &'static str) -> (String, std::sync::mpsc::Receiver<String>) {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let _ = tx.send(String::from_utf8_lossy(&buf[..n]).to_string());
                let body = "rail-ref-123";
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            });
            (format!("http://{addr}/pay"), rx)
        }

        // 2xx → Ok, with the idempotency key on the wire.
        let (url, rx) = serve_once("200 OK");
        let ok = HttpPaymentProvider::new(url, Some("tok".into()))
            .pay("acme-vendor", 200, "flint-abc123")
            .expect("2xx confirms the charge");
        assert_eq!(ok, "rail-ref-123", "the rail reference comes back");
        let req = rx.recv().unwrap();
        assert!(
            req.contains("idempotency-key: flint-abc123")
                || req.contains("Idempotency-Key: flint-abc123"),
            "the idempotency key reaches the wire: {req}"
        );
        assert!(req.contains("\"payee\":\"acme-vendor\"") && req.contains("\"amount\":200"));

        // 4xx → the order was REJECTED before processing: provably not charged.
        let (url, _rx) = serve_once("422 Unprocessable Entity");
        assert!(matches!(
            HttpPaymentProvider::new(url, None).pay("acme-vendor", 200, "k"),
            Err(PayError::NotCharged(_))
        ));

        // 5xx → the order REACHED the rail and then something broke: indeterminate.
        let (url, _rx) = serve_once("500 Internal Server Error");
        assert!(matches!(
            HttpPaymentProvider::new(url, None).pay("acme-vendor", 200, "k"),
            Err(PayError::Indeterminate(_))
        ));

        // Connection refused → nothing was ever sent: provably not charged.
        let dead = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let a = l.local_addr().unwrap();
            drop(l);
            format!("http://{a}/pay")
        };
        assert!(matches!(
            HttpPaymentProvider::new(dead, None).pay("acme-vendor", 200, "k"),
            Err(PayError::NotCharged(_))
        ));

        // Timeout after the request was sent → indeterminate (the charge may have posted).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1500));
            drop(stream); // accept, read nothing back, never respond
        });
        assert!(matches!(
            HttpPaymentProvider::new(format!("http://{addr}/pay"), None)
                .with_timeout(std::time::Duration::from_millis(300))
                .pay("acme-vendor", 200, "k"),
            Err(PayError::Indeterminate(_))
        ));

        // A REDIRECT is never followed (council S29 F6): the default policy would re-issue the
        // POST as a GET whose 200 mints a Performed receipt off a login page. 3xx ⇒ indeterminate.
        let (url, _rx) = serve_once("302 Found");
        assert!(matches!(
            HttpPaymentProvider::new(url, None).pay("acme-vendor", 200, "k"),
            Err(PayError::Indeterminate(_))
        ));

        // A malformed endpoint (builder error — nothing ever left the process) is provably NOT
        // charged (council S29 F3), never "the charge may have posted".
        assert!(matches!(
            HttpPaymentProvider::new("not a url at all".to_string(), None).pay(
                "acme-vendor",
                200,
                "k"
            ),
            Err(PayError::NotCharged(_))
        ));
    }

    /// Council S29 red-team F2: concurrent in-flight payments are BOUNDED fail-closed — the 9th
    /// while 8 block on the rail is REFUSED with nothing debited and nothing sent; slots release
    /// when the rail returns.
    #[test]
    fn pay_in_flight_concurrency_is_bounded_fail_closed() {
        struct BlockingProvider {
            entered: std::sync::atomic::AtomicUsize,
            rx: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        }
        impl PaymentProvider for BlockingProvider {
            fn rail(&self) -> crate::payment_ledger::PaymentRail {
                crate::payment_ledger::PaymentRail::Unknown
            }
            fn pay(&self, _payee: &str, _amount: u64, _key: &str) -> Result<String, PayError> {
                self.entered
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = self.rx.lock().unwrap().recv(); // hold the slot until released
                Ok("ok".to_string())
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let provider = Arc::new(BlockingProvider {
            entered: std::sync::atomic::AtomicUsize::new(0),
            rx: std::sync::Mutex::new(rx),
        });
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 1000).unwrap();
        let reg = Arc::new(
            MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None).with_payments(
                meter.clone(),
                provider.clone(),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            ),
        );
        let mut handles = Vec::new();
        for _ in 0..8 {
            let r = reg.clone();
            handles.push(std::thread::spawn(move || {
                r.execute(&pay_intent("acme-vendor", "vm-agent", "10"))
            }));
        }
        // Wait until all 8 are genuinely in-flight (inside the rail call, slots held).
        while provider.entered.load(std::sync::atomic::Ordering::SeqCst) < 8 {
            std::thread::yield_now();
        }
        match reg.execute(&pay_intent("acme-vendor", "vm-agent", "10")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("in-flight"),
                    "the 9th concurrent payment is refused by the bound: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
        assert_eq!(
            meter.remaining("vm-agent"),
            1000 - 80,
            "the refused payment debited NOTHING (only the 8 in-flight reservations)"
        );
        for _ in 0..8 {
            tx.send(()).unwrap();
        }
        for h in handles {
            assert!(matches!(
                h.join().unwrap(),
                IntentExecution::Performed { .. }
            ));
        }
        // Slots released: a new payment goes through again.
        drop(tx);
        assert!(matches!(
            reg.execute(&pay_intent("acme-vendor", "vm-agent", "10")),
            IntentExecution::Performed { .. }
        ));
    }

    /// The idempotency key is derived from the intent's SIGNATURE — unique per signed declaration
    /// (an intent_id can recycle out of the replay window; a signature cannot), so the rail can
    /// dedupe without ever double-moving money for one signed intent.
    #[test]
    fn pay_idempotency_key_is_signature_derived_and_unique_per_signed_intent() {
        #[derive(Default)]
        struct KeyRecordingProvider(std::sync::Mutex<Vec<String>>);
        impl PaymentProvider for KeyRecordingProvider {
            fn rail(&self) -> crate::payment_ledger::PaymentRail {
                crate::payment_ledger::PaymentRail::Unknown
            }
            fn pay(&self, _payee: &str, _amount: u64, key: &str) -> Result<String, PayError> {
                self.0.lock().unwrap().push(key.to_string());
                Ok("ok".to_string())
            }
        }
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 100).unwrap();
        let provider = Arc::new(KeyRecordingProvider::default());
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter,
                provider.clone(),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        let i1 = pay_intent("acme-vendor", "vm-agent", "10");
        let i2 = pay_intent("acme-vendor", "vm-agent", "10"); // same shape, fresh key+signature
        assert!(matches!(
            reg.execute(&i1),
            IntentExecution::Performed { .. }
        ));
        assert!(matches!(
            reg.execute(&i2),
            IntentExecution::Performed { .. }
        ));
        let keys = provider.0.lock().unwrap().clone();
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0], format!("flint-{}", i1.signature));
        assert_eq!(keys[1], format!("flint-{}", i2.signature));
        assert_ne!(
            keys[0], keys[1],
            "distinct signed intents get distinct keys"
        );
    }

    /// Council S28 F3: when the rail fails AND the durable refund cannot persist, the signed reason
    /// must NOT claim "spend refunded" — the honest record is that the cap remains debited. The
    /// provider itself destroys the meter's directory mid-payment (between the persisted
    /// reservation and the refund), the only real window this can happen in.
    #[test]
    fn pay_rail_failure_with_unpersistable_refund_is_recorded_honestly() {
        struct DirDestroyingFailingProvider(std::path::PathBuf);
        impl PaymentProvider for DirDestroyingFailingProvider {
            fn rail(&self) -> crate::payment_ledger::PaymentRail {
                crate::payment_ledger::PaymentRail::Unknown
            }
            fn pay(&self, _payee: &str, _amount: u64, _key: &str) -> Result<String, PayError> {
                std::fs::remove_dir_all(&self.0).expect("kill the meter's persist target");
                Err(PayError::NotCharged("rail unavailable".to_string()))
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("meter");
        std::fs::create_dir(&sub).unwrap();
        let meter = Arc::new(
            elastos_runtime::primitives::spend::SpendMeter::open_durable(
                sub.join("spend_meter.json"),
            )
            .unwrap(),
        );
        meter.set_budget("vm-agent", 100).unwrap();
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None)
            .with_payments(
                meter.clone(),
                Arc::new(DirDestroyingFailingProvider(sub)),
                Arc::new(crate::payment_ledger::PaymentLedger::new()),
            );
        match reg.execute(&pay_intent("acme-vendor", "vm-agent", "40")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("the cap remains debited"),
                    "the signed reason must say the cap remains debited, got: {reason}"
                );
                assert!(
                    !reason.contains("spend refunded"),
                    "the signed reason must NOT claim a refund that is not in force: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
        assert_eq!(
            meter.remaining("vm-agent"),
            60,
            "the unpersistable refund was rolled back — the cap really does remain debited"
        );
    }

    /// Fail-closed scoping: outside the pay namespace, a bad payee, a non-integer amount, a zero
    /// amount, or a NON-CANONICAL amount (leading zero / sign / space — F4) all DECLINE, and none
    /// debits the cap. (Canonical form is required so a Performed pay always reconciles Matched.)
    #[test]
    fn pay_declines_bad_scope_and_amount() {
        let meter = Arc::new(SpendMeter::new());
        meter.set_budget("vm-agent", 1000).unwrap();
        let (reg, provider) = pay_registry(meter.clone());
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let bad = |resource: &str, amount: &str| {
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "pi",
                "vm-agent",
                "runtime.pay",
                amount,
                resource,
                "execute",
                "grant-1",
            )
        };
        for (resource, amount) in [
            ("elastos://mail/send".to_string(), "10".to_string()), // outside namespace
            (PAY_PREFIX.to_string(), "10".to_string()),            // empty payee
            (format!("{PAY_PREFIX}a/b"), "10".to_string()),        // path trick in payee
            (format!("{PAY_PREFIX}acme"), "not-a-number".to_string()), // non-integer amount
            (format!("{PAY_PREFIX}acme"), "0".to_string()),        // zero amount
            (format!("{PAY_PREFIX}acme"), "0200".to_string()), // non-canonical (leading zero) — F4
            (format!("{PAY_PREFIX}acme"), "+200".to_string()), // non-canonical (sign) — F4
            (format!("{PAY_PREFIX}acme"), " 200".to_string()), // non-canonical (space) — F4
        ] {
            assert!(
                matches!(
                    reg.execute(&bad(&resource, &amount)),
                    IntentExecution::Declined { .. }
                ),
                "must decline resource {resource:?} amount {amount:?}"
            );
        }
        assert_eq!(
            meter.remaining("vm-agent"),
            1000,
            "no declined pay ever touched the cap"
        );
        assert!(provider.payments.lock().unwrap().is_empty());
    }

    /// Without a wired meter/provider, `runtime.pay` is honestly UNWIRED ⇒ Declined ⇒ Undelivered.
    #[test]
    fn pay_is_unwired_without_with_payments() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        assert!(matches!(
            reg.execute(&pay_intent("acme-vendor", "vm-agent", "10")),
            IntentExecution::Declined { .. }
        ));
    }

    /// A write the store cannot persist DECLINES with the true reason — Performed only for a write
    /// that landed. (Seam: a FILE squatting where the store's directory tree must be created.)
    #[test]
    fn state_put_declines_when_the_store_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Local"), b"squat").unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{STATE_PUT_PREFIX}cursor");
        match reg.execute(&state_put_intent(&resource, "vm-agent", "cafe01")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("could not be persisted"),
                    "true reason: {reason}"
                );
            }
            other => panic!("an unlanded write must Decline, got {other:?}"),
        }
    }

    /// Without a data dir there is no Inbox store to deliver into — the method is honestly
    /// UNWIRED (⇒ Undelivered), never a fabricated delivery.
    #[test]
    fn notify_is_unwired_without_a_data_dir() {
        let reg = MethodRegistryExecutor::production(Arc::new(AuditLog::new()), None);
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        assert!(matches!(
            reg.execute(&notify_intent(&resource, "vm-agent", "")),
            IntentExecution::Declined { .. }
        ));
    }

    /// A delivery the store cannot persist is DECLINED with the true reason — Performed is only
    /// ever reported for a write that landed. (Seam: a FILE squatting where the notifications
    /// directory tree must be created makes the store write fail, root or not.)
    #[test]
    fn notify_declines_when_the_store_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        // The notifications store lives under <data_dir>/Local/... — squat a FILE at Local.
        std::fs::write(dir.path().join("Local"), b"squat").unwrap();
        let reg = MethodRegistryExecutor::production(
            Arc::new(AuditLog::new()),
            Some(dir.path().to_path_buf()),
        );
        let resource = format!("{INBOX_NOTIFY_PREFIX}agent-status");
        match reg.execute(&notify_intent(&resource, "vm-agent", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("could not be delivered"),
                    "the true failure is named: {reason}"
                );
            }
            other => panic!("an unlanded delivery must Decline, got {other:?}"),
        }
    }

    #[test]
    fn content_seen_tracks_real_state_not_the_declaration() {
        use elastos_runtime::capability::IntentDeclarationV1;
        let dir = tempfile::tempdir().unwrap();
        let log = Arc::new(AuditLog::with_file(dir.path().join("audit.log")).unwrap());
        // Record that principal "vm-agent" SUCCESSFULLY OPENED one content id.
        log.content_open("sess", "vm-agent", "QmSEEN", "view", "opened", "prov", None)
            .unwrap();
        let reg = MethodRegistryExecutor::production(log, None);

        // Intent resource is a content-access-CHECK ref: prefix + content id.
        let check = |content_id: &str| format!("{CONTENT_ACCESS_CHECK_PREFIX}{content_id}");
        let intent_for = |resource: String, capsule: &str| {
            let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
            IntentDeclarationV1::issue(
                &sk,
                sk.verifying_key().to_bytes(),
                "i",
                capsule,
                "runtime.content_seen",
                "",
                &resource,
                "read",
                "grant-1",
            )
        };
        // The SAME method + declaration shape reconciles differently based on REAL state:
        match reg.execute(&intent_for(check("QmSEEN"), "vm-agent")) {
            IntentExecution::Performed {
                resource, action, ..
            } => {
                assert_eq!(resource, check("QmSEEN")); // the CHECK resource, honestly echoed
                assert_eq!(action, "read");
            }
            other => panic!("expected Performed for a seen content id, got {other:?}"),
        }
        // Never-opened id ⇒ Declined.
        assert!(matches!(
            reg.execute(&intent_for(check("QmNEVER"), "vm-agent")),
            IntentExecution::Declined { .. }
        ));
        // PRINCIPAL-SCOPED: a DIFFERENT capsule asking about the same id gets Declined — no
        // cross-principal existence oracle.
        assert!(
            matches!(
                reg.execute(&intent_for(check("QmSEEN"), "vm-other")),
                IntentExecution::Declined { .. }
            ),
            "content_seen must not reveal another principal's access"
        );
    }
    // ─────────────────────── runtime.market_quote (Sprint 39) ───────────────────────

    struct ScriptedQuoter(Result<crate::api::buy_authority::BuyQuote, String>);
    impl crate::market_quote::MarketQuoter for ScriptedQuoter {
        fn quote(&self, _: &str) -> Result<crate::api::buy_authority::BuyQuote, String> {
            self.0.clone()
        }
    }

    fn quote_intent(asset: &str, declared: &str) -> IntentDeclarationV1 {
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "quote-intent-1",
            "vm-shopper",
            "runtime.market_quote",
            declared,
            &format!("{PAY_PREFIX}{asset}"),
            "read",
            "grant-1",
        )
    }

    fn quote_registry(
        outcome: Result<crate::api::buy_authority::BuyQuote, String>,
    ) -> MethodRegistryExecutor {
        MethodRegistryExecutor::new()
            .with_market_quotes(Arc::default(), Arc::new(ScriptedQuoter(outcome)))
    }

    fn terms_5_usdc() -> crate::api::buy_authority::BuyQuote {
        crate::api::buy_authority::BuyQuote {
            price: "5000000".to_string(),
            pay_token: "0xUSDC".to_string(),
            supply: 3,
        }
    }

    /// Discovery mode ("" declared): the read performs AS DECLARED (input_hash echo "") so it
    /// reconciles Matched, and the terms reach the agent ONLY via the explicit disclosure.
    #[test]
    fn market_quote_discovery_returns_terms_via_the_disclosure_channel() {
        let exec = quote_registry(Ok(terms_5_usdc()));
        match exec.execute(&quote_intent("QmMovie", "")) {
            IntentExecution::Performed {
                input_hash,
                action,
                agent_visible_report,
                rail_ref,
                ..
            } => {
                assert_eq!(input_hash, "", "discovery echoes the declaration — Matched");
                assert_eq!(action, "read");
                assert_eq!(
                    agent_visible_report.as_deref(),
                    Some("price=5000000;tok=0xUSDC;supply=3"),
                    "the terms travel via the EXPLICIT disclosure channel"
                );
                assert!(rail_ref.is_none(), "a quote settles nothing — no rail_ref");
            }
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    /// Attested mode: declaring the CURRENT terms Matches (the echo equals the declaration);
    /// declaring STALE terms gets the ACTUAL terms echoed — the reconciliation Diverges, never a
    /// fabricated match.
    #[test]
    fn market_quote_attested_mode_echoes_actual_terms() {
        let exec = quote_registry(Ok(terms_5_usdc()));
        let current = "price=5000000;tok=0xUSDC;supply=3";
        match exec.execute(&quote_intent("QmMovie", current)) {
            IntentExecution::Performed { input_hash, .. } => {
                assert_eq!(
                    input_hash, current,
                    "believed-correct terms reconcile Matched"
                );
            }
            other => panic!("expected Performed, got {other:?}"),
        }
        match exec.execute(&quote_intent("QmMovie", "price=1;tok=0xUSDC;supply=3")) {
            IntentExecution::Performed { input_hash, .. } => {
                assert_eq!(
                    input_hash, current,
                    "stale belief: the ACTUAL terms are echoed — Diverged, never fabricated"
                );
            }
            other => panic!("expected Performed, got {other:?}"),
        }
    }

    /// A failed read (no listing / chain unreachable) DECLINES with the bounded error — a quote
    /// is performed only when it truly returned terms. And the pay namespace is the boundary:
    /// a non-pay resource declines before any read.
    #[test]
    fn market_quote_declines_on_read_failure_and_outside_the_pay_namespace() {
        let exec = quote_registry(Err("no active listing".to_string()));
        match exec.execute(&quote_intent("QmMovie", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("no active listing"),
                    "honest reason: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
        let sk = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
        let outside = IntentDeclarationV1::issue(
            &sk,
            sk.verifying_key().to_bytes(),
            "quote-intent-2",
            "vm-shopper",
            "runtime.market_quote",
            "",
            "elastos://runtime/state/secret-key",
            "read",
            "grant-1",
        );
        match quote_registry(Ok(terms_5_usdc())).execute(&outside) {
            IntentExecution::Declined { reason } => {
                assert!(reason.contains("must be"), "namespace-bounded: {reason}");
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }

    /// The affordance rides the ONE quote spine: a cached quote is served with NO quoter call,
    /// and a fresh in-flight claim by another consumer declines rather than duplicating the read.
    #[test]
    fn market_quote_shares_the_single_flight_cache() {
        struct CountingQuoter(std::sync::atomic::AtomicUsize);
        impl crate::market_quote::MarketQuoter for CountingQuoter {
            fn quote(&self, _: &str) -> Result<crate::api::buy_authority::BuyQuote, String> {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(crate::api::buy_authority::BuyQuote {
                    price: "7".to_string(),
                    pay_token: "native".to_string(),
                    supply: 1,
                })
            }
        }
        let cache: crate::market_quote::MarketQuoteCache = Arc::default();
        let quoter = Arc::new(CountingQuoter(std::sync::atomic::AtomicUsize::new(0)));
        let exec = MethodRegistryExecutor::new().with_market_quotes(cache.clone(), quoter.clone());
        assert!(matches!(
            exec.execute(&quote_intent("QmA", "")),
            IntentExecution::Performed { .. }
        ));
        assert!(matches!(
            exec.execute(&quote_intent("QmA", "")),
            IntentExecution::Performed { .. }
        ));
        assert_eq!(
            quoter.0.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the second quote is a cache hit — one live read per asset per window"
        );
        // Another consumer (the panel) holds the in-flight claim for QmB ⇒ the agent's quote
        // declines with retry rather than duplicating the chain read.
        crate::market_quote::claim_or_serve(&cache, "QmB", crate::market_quote::now_unix(), true);
        match exec.execute(&quote_intent("QmB", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("in progress"),
                    "single-flight respected: {reason}"
                );
            }
            other => panic!("expected Declined, got {other:?}"),
        }
    }
    /// Council S39 fold (red-team F2): terms the QUOTE SOURCE returns are held to the same bound
    /// as the agent's declaration before they are signed/disclosed — out-of-bound terms refuse,
    /// they never land on the chain or in the response.
    #[test]
    fn market_quote_refuses_malformed_terms_from_the_quote_source() {
        let huge = quote_registry(Ok(crate::api::buy_authority::BuyQuote {
            price: "9".repeat(500),
            pay_token: "0xUSDC".to_string(),
            supply: 1,
        }));
        match huge.execute(&quote_intent("QmMovie", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("malformed terms"),
                    "bounded refusal: {reason}"
                );
            }
            other => panic!("expected Declined on oversized terms, got {other:?}"),
        }
        let nonprintable = quote_registry(Ok(crate::api::buy_authority::BuyQuote {
            price: "5\u{7}00".to_string(), // a control byte from a hostile quote source
            pay_token: "0xUSDC".to_string(),
            supply: 1,
        }));
        match nonprintable.execute(&quote_intent("QmMovie", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("malformed terms"),
                    "charset refusal: {reason}"
                );
            }
            other => panic!("expected Declined on non-printable terms, got {other:?}"),
        }
    }

    /// Council S39 fold (red-team F1): concurrent quote reads are BOUNDED like payments — with
    /// every slot parked on a hung read, the next quote refuses with retry instead of parking
    /// another blocking thread; released slots admit again.
    #[test]
    fn market_quote_inflight_reads_are_bounded_fail_closed() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::mpsc;

        /// A quoter that parks until released, counting entries.
        struct ParkedQuoter {
            entered: AtomicUsize,
            release: std::sync::Mutex<mpsc::Receiver<()>>,
        }
        impl crate::market_quote::MarketQuoter for ParkedQuoter {
            fn quote(&self, _: &str) -> Result<crate::api::buy_authority::BuyQuote, String> {
                self.entered.fetch_add(1, Ordering::SeqCst);
                // Park until the test releases us (bounded by the recv timeout below).
                let _ = self
                    .release
                    .lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(30));
                Ok(crate::api::buy_authority::BuyQuote {
                    price: "1".to_string(),
                    pay_token: "native".to_string(),
                    supply: 1,
                })
            }
        }
        let (tx, rx) = mpsc::channel::<()>();
        let quoter = Arc::new(ParkedQuoter {
            entered: AtomicUsize::new(0),
            release: std::sync::Mutex::new(rx),
        });
        let exec = Arc::new(
            MethodRegistryExecutor::new().with_market_quotes(Arc::default(), quoter.clone()),
        );

        // Park 8 reads on 8 DISTINCT assets (single-flight dedups per asset, so distinct assets
        // are what can stack threads — exactly the attack the bound closes).
        let mut handles = Vec::new();
        for i in 0..8 {
            let exec = exec.clone();
            handles.push(std::thread::spawn(move || {
                exec.execute(&quote_intent(&format!("Qm{i}"), ""))
            }));
        }
        // Wait until all 8 are genuinely inside the quoter (parked on the channel).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while quoter.entered.load(Ordering::SeqCst) < 8 {
            assert!(
                std::time::Instant::now() < deadline,
                "parked readers never arrived"
            );
            std::thread::yield_now();
        }

        // The 9th DISTINCT asset: every slot is parked ⇒ refuse with retry, never a 9th thread.
        match exec.execute(&quote_intent("Qm-ninth", "")) {
            IntentExecution::Declined { reason } => {
                assert!(
                    reason.contains("already") && reason.contains("in-flight"),
                    "the bound refuses, fail-closed: {reason}"
                );
            }
            other => panic!("expected the in-flight bound to refuse, got {other:?}"),
        }
        assert_eq!(
            quoter.entered.load(Ordering::SeqCst),
            8,
            "the 9th quote never reached the quote source"
        );

        // Release the parked readers; the slots free and a new quote is admitted again.
        for _ in 0..8 {
            let _ = tx.send(());
        }
        for h in handles {
            let _ = h.join().unwrap();
        }
        // Disconnect the channel so the admitted after-quote returns immediately instead of
        // parking out its full recv timeout.
        drop(tx);
        match exec.execute(&quote_intent("Qm-after", "")) {
            IntentExecution::Performed { .. } => {}
            other => panic!("released slots must admit again, got {other:?}"),
        }
    }
}
