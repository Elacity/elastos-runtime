# Flint — The Mandate Engine

**"Give your agent a mandate, not your keys."**

The single reviewable map of the accountability engine delivered on this branch
(`claude/git-proxy-auth-roadmap-c214hu`) for merge into `flint-0.5`. It is the layer that lets an
operator hand an AI agent a *scoped, expiring, revocable* mandate instead of raw credentials, have
the agent act under it unsupervised, and hold the whole thing to a tamper-evident, off-box-verifiable
record. Detailed per-gap history lives in `KNOWN_GAPS.md`; this is the summary.

## The lifecycle, in one place

| Verb | What it is | Where |
|---|---|---|
| **Grant** | Mint a real signed capability token scoped to (capsule, resource, action, ttl) + an authorized method set + the **responsible entity** (the DID the OPERATOR DECLARES accountable for the agent's acts — the EU-AI-Act liability *attribution*, signed into the grant record and the receipt; required on the shell app). **Operator-asserted, not attested:** the runtime records the DID verbatim; it does not resolve it or obtain the entity's counter-signature, so a receipt proves the operator's declaration, not the entity's consent (entity counter-signing is a tracked gap). | CLI + Mandates shell app |
| **Act** | An agent signs an `IntentDeclarationV1` and dispatches it; a fail-closed gate checks `intent ⊆ envelope` (capsule + agent-key + method + resource + action), runs a real executor, and reconciles declared-vs-done | `POST /api/agent/dispatch` (agent-facing, S26) or `/api/standing-grants/dispatch` (operator) |
| **Watch** | The operator sees mandates live, and what each agent has written/delivered under them | Mandates shell app (mandate cards, Agent State panel, Inbox) |
| **Revoke** | The kill switch — durably attested *before* the mandate dies | CLI + Mandates shell app |
| **Prove** | Export a portable `MandateReceipt` — proving not just WHAT the agent did but WHICH entity the operator DECLARED accountable (the responsible entity rides the signed grant record; operator-asserted, see Grant) — and verify it off-box with no runtime and no trust in this box. **Verifier version floor (S32):** a pre-S32 `verify-receipt` binary drops the new grant field on re-serialize and will false-flag an S32 receipt as tampered — verify with an S32+ binary. | `elastos verify-receipt` |

## The real affordances behind dispatch

An affordance is a genuine runtime operation an agent can invoke under a mandate. Each REPORTS what
it actually did; the receipt is minted from that report, never from the declaration, so an
authorized-but-unperformed act reconciles honestly (`authorized_not_performed`), never a fabricated
match (this is the G-M6 rule).

| Method | Kind | Reports | Notes |
|---|---|---|---|
| `runtime.audit_verify` | read (side-effect-free) | `read` | Re-verifies the signed audit chain end to end; `performed` iff it truly verifies |
| `runtime.content_seen` | state-dependent read | `read` | Did THIS principal open a content id? Principal-scoped, no cross-principal oracle |
| `runtime.state_get` | attested VERIFY read | `read` | Verifies the acting principal's OWN durable state (the read pair of state_put); the agent declares the value it expects — `matched` attests "K = V", `diverged` means the guess was wrong (ONE BIT — the actual value is NOT returned or on-chain), `declined` if absent. Principal-scoped, exact-key, agent-key BOUND (F2) |
| `runtime.notify` | **side-effecting** | `message` | Delivers a message into the operator's Inbox; bounded fields, capped store, `performed` only after the write lands |
| `runtime.state_put` | **side-effecting** | `write` | Writes durable, readable-back, principal-scoped agent state; last-write-wins with attributed versioning |
| `runtime.pay` | **side-effecting, money** | `execute` | Spends real money to a mandate-scoped payee, capped by the spend meter (S27). The amount rides in the signed `input_hash` (canonical decimal); over the cap (or an unprovisioned capsule) the payment is REFUSED with no money moved (a signed `authorized_not_performed`); a PROVABLY-not-charged rail refusal refunds the reservation (indeterminate outcomes keep it — see S29). Rail-agnostic (`PaymentProvider` — card/ACH/Stripe, or a crypto rail; **cryptography, not cryptocurrency** — no rail is privileged). Opt-in via `with_payments`. **S28:** the cap is DURABLE (snapshot+fsync; the reservation persists BEFORE money moves; a restart never refills it; a corrupt/tampered-shape snapshot refuses to boot) and operator-provisionable at `POST /api/spend-budgets` (shell-only; refused without a wired rail or on a non-durable meter; attested on the signed chain as a `ConfigChange`, rolled back — a first-time provision fully removed — if the attestation fails; a correlated double persist failure is surfaced loudly, never hidden). **S29 — the REAL rail:** `ELASTOS_PAYMENT_ENDPOINT` (+ optional bearer `ELASTOS_PAYMENT_TOKEN`) wires `HttpPaymentProvider` (https enforced — plaintext only to loopback; malformed endpoints refuse at boot; redirects never followed — a 3xx is indeterminate, never "charged") — a payment order (`payee`, `amount`, signature-derived `Idempotency-Key`) POSTed to the deployment's payment service (a thin adapter fronts Stripe/ACH/treasury/a crypto rail), REQUIRING the durable meter. Outcomes are classified two-generals-honestly: 2xx = charged; 4xx/never-connected = provably-not-charged ⇒ the reservation is REFUNDED; timeout/5xx/panic = **INDETERMINATE** ⇒ the reservation is KEPT (refunding against money that may have moved would break the cap — the one unbreakable invariant) and the DECLINE REASON — surfaced in the dispatch response and error-logged (on-chain reasons are the tracked follow-on) — names the idempotency key for rail reconciliation; `authorized_not_performed` here means NOT-ATTESTED, not proven-absent. Concurrent in-flight payments are bounded fail-closed. The meter POISONS on a post-publish persist failure (mutations refuse until reopened; no divergence) and holds a single-opener flock on its snapshot. **Honest bounds (council):** the Mock rail stays dev/demo-gated (`ELASTOS_ALLOW_MOCK_PAYMENTS`; the real endpoint wins if both are set); the cap is PER-CAPSULE not per-mandate; an orphaned/indeterminate reservation over-counts the cap fail-closed (recovery: rail-side lookup by idempotency key FIRST, then the operator raising the limit — a blind cap raise after indeterminate drain can authorize real spend beyond the original intent; **S30:** the payment LEDGER durably custodies every rail attempt THE PROCESS LIVED TO RECORD (a crash between reservation and rail verdict still leaves the S29 orphaned reservation, recovered from the on-chain declaration; pending custody is guaranteed-or-stated in the reason, terminal records best-effort) — the performed payment's rail reference, and a PENDING entry per indeterminate outcome (per-capsule bounded, so one agent cannot blind a victim's obligations) — surfaced at `GET /api/payments/pending` and resolved EXACTLY ONCE at `POST /api/payments/reconcile` (shell-only; `charged=false` refunds the reservation, `charged=true` confirms it; each resolution attested on the signed chain); the budget surface shows `pending_units` held-unconfirmed distinct from confirmed spend, and dispatch runs on the blocking pool (rail latency never starves the async workers). The endpoint's obligations are the stated contract in `docs/PAYMENT_ENDPOINT_CONTRACT.md`; an AUTOMATED reconciliation loop is the tracked follow-on. **S31 — the Money panel:** budgets (cap/spent/remaining/held-unconfirmed, the poisoned banner) and the reconciliation work list live in the Mandates shell app, every pixel a read-only projection of the ONE enforcing meter+ledger (`build_pay_rail`, same-Arc by construction, flock-enforced); the web provisioning surface is CEILING-BOUND server-side (`ELASTOS_WEB_MAX_SPEND_CAP`, mirroring the issue route's narrowing — an XSS in the frame cannot provision an unbounded cap), verdict buttons are arm→confirm like the kill switch, a not-charged verdict is refused while the meter is poisoned (it would burn the one-shot refund handle), and the S31 residual (the 12h URL-borne launch token) is CLOSED by **S33 — the money-authorization perimeter**: the mandates launch token is delivered via an HttpOnly, SameSite=Strict cookie path-scoped to the mandates API (the launch URL carries no credential — only a non-secret `shell=1` marker), cookie-authorized writes require the anti-CSRF app-marker header, and the money writes (set-cap, reconcile) each require a FRESH passkey verification (a WebAuthn ceremony ≤180s old, proof-bound to the same principal) that is SPENT on exactly one applied write (keyed on the canonical signed payload, so re-encoding the token string does not mint a fresh spend; a provably pre-effect refusal re-credits the ceremony) — replaying it on the same or the other money verb is refused. What the fresh binding proves is exactly: THIS operator's authenticator freshly approved ONE money write — no more (it does not verb-scope at mint; single-use consumption is what stops a second write). Honest bounds: the spent-token guard is in-memory (a gateway restart inside the ~3-minute window could admit one replay), and a runtime with no passkey enrolled makes money writes CLI-only (stated in the panel) — both tracked as G-M9. The 4xx⇒not-charged rule is a stated CONTRACT on the payment endpoint; the snapshot file is trusted from `data_dir` (not self-authenticating, unlike the signed chain); the flock/parent-fsync protections are unix-only (elsewhere the serve/gateway host lock is the bound); a lying 2xx from a compromised endpoint mints receipts the runtime cannot independently check — a Performed pay is a RAIL-TRUST attestation, weaker than the runtime-verified affordances. **S34 — the DRM wedge (the rail IS the marketplace):** `ELASTOS_PAYMENT_RAIL=drm` wires `DrmMarketplaceProvider` behind the SAME `PaymentProvider` trait — a `runtime.pay` act whose payee names a DRM asset SETTLES ON-CHAIN via the Elacity `buy_authority` path instead of an HTTPS POST, with the meter, ledger, two-generals classification, and receipt byte-identical to the HTTP rail (one pay spine, never a fork). The KID→`(operative, tokenId)` binding goes through the MKT-1-hardened resolver, FAIL-CLOSED on ambiguity (an ambiguous or unresolvable asset is `NotCharged`/refunded — NEVER a fallback buy); a provably-not-broadcast buy refunds; a broadcast-then-unconfirmed outcome is INDETERMINATE (reservation kept, reconciled by the intent-signature idempotency key mapped to the tx). The settlement's on-chain truth — the tx hash + `operative:tokenId` — is carried on the signed `CapabilityUse` as `rail_ref` (S34; back-compat identical to S32: appended last, `skip_serializing_if`, byte-identical re-serialization of pre-S34 chains) and thus into the portable receipt, so `verify-receipt` shows WHICH tx settled the mandate's payment. The chain boundary is behind two small injected traits (`DrmResolver`/`DrmSettler`) — CI exercises every branch with mocks; the live Base path is an operator runbook step, never a CI call. Honest bounds (tracked MKT-DRM): the spend-meter cap is in SPEND UNITS while the on-chain price is the listing's token amount (the meter-unit ⇄ price reconciliation is not yet enforced — the cap bounds intent, the listing bounds the actual charge); a Performed DRM buy trusts the buy path's tx report (the runtime does not yet re-verify the receipt/confirmations depth on-chain); royalty-split correctness is the DRM protocol's, not re-verified here |

## The trust model (and its honest caveats)

- **The runtime is the single audit writer.** Trust in a receipt derives from a signed `did:key`
  pinned out-of-band; verification is fully off-box (`elastos verify-receipt --signer`).
- **The shell is the grant root (G-M3).** Issuing/revoking authority lives behind the shell's
  consent-broker gate (API) and the app-bound home-launch token (gateway) — the same trust tier. A
  compromised shell can already mint anything, so shell-held mandate power is *contained*, not new.
- **Agent-key binding is optional today (G-M4).** A mandate MAY bind one agent's ed25519 key
  (strong attribution); unbound, it is capsule-string-only. Promoting binding to default is tracked.
- **Same-disk / same-host caveats, stated not hidden:** durable stores are fail-closed on corrupt
  boot but not defended against a root attacker who already owns the key material; the replay guard's
  freshness window trusts the host clock (fail-closed both directions — see below).

## Gaps closed vs open (mandate track)

**Closed:** G-M1 (liveness consults every kill path), G-M2 (token-keyed `CapabilityUse` in the
receipt), G-M5 (durable registry + replay guard survive restart and power loss), G-M6 (the
reconciliation seam — receipts minted from what executors report).

**Agent-facing dispatch shipped (S26):** the ACT leg is now reachable by the AGENT itself at
`/api/agent/dispatch` — not only the operator shell. The agent authenticates AS the mandate holder
(the signed intent proves key possession; the mandate's agent-key binding proves authorization), so
NO operator session/keys are needed — the literal "a mandate, not your keys". The route requires a
BOUND mandate (no ambient authority), refuses a wrong-key/unbound/absent mandate with a uniform 403,
and checks the binding BEFORE the rate budget (charge-on-authorized — a wrong-key flood can't lock
out a victim). The operator shell route remains for operator-driven dispatch and unbound mandates.

**Open / tracked (by design or roadmap):**
- **G-M3** — shell is the grant root (accepted trust model).
- **G-M4** — agent-key binding optional; REQUIRED on the agent-facing dispatch route + for state_get
  mandates. Promoting binding to the universal default (every side-effecting/state affordance) is the
  tracked endpoint.
- **G-M7** — operational hardening. *Paid down:* the replay guard is time-windowed + bounded +
  clock-attack-hardened (S19); dispatch is rate-budgeted + grant-existence-gated (S21) with a
  per-mandate configurable budget (S22); and the working registry's DEAD accumulation is now bounded
  on both axes — time-retention + hard cap that never sheds a live mandate (S23). Durable *dead*
  state (replay set, dispatch rate, dead grants) is bounded; *live* mandate growth stays real
  operator authority by design (never shed). Mandate mint is now FAIL-CLOSED (S24): `grant_durable`
  emits the signed `CapabilityGrant` before returning the token, so a mandate whose grant cannot be
  recorded is never issued — the receipt is a complete record of every issued mandate's GRANT and act
  DECLARATION (reconciliation verdicts remain best-effort — the disclosed executor-report seam). This
  holds as durably as the audit log's backing: fully under the file-backed EU-AI-Act mode, in-process
  under the default memory-only log (the same bound as the fail-closed revoke). *Remaining:*
  a request-RATE limiter on the gateway mint/revoke routes; and the principled fix for one-click
  broad grants is role-based capability tiering (a `CapsuleRole::System`), a separate initiative
  deliberately NOT wedged in on a spoofable capsule name.

## The replay guard (security core)

Each signed intent acts at most once. The guard is **durable** (survives restart/power loss),
**time-windowed** (a captured declaration expires after `MAX_INTENT_AGE_SECS`; future-dated ones are
refused), **bounded** (the seen-set self-compacts against the retention window instead of growing
forever), and **clock-attack-hardened** (a persisted, monotonic anti-readmit watermark refuses any
intent at/below the highest evicted `declared_at`, so a backward clock step cannot readmit a
compacted id — under any clock, across restart). `RETENTION = age + skew` is the load-bearing margin.

## Review discipline

Every sprint (13 mandate-engine sprints on this branch, on top of the receipt/CLI foundation) ran
the same standard before commit: **gate** (build + clippy + full test suites) → **principles
guardian** review → **red-team** review → fold findings → commit → push. Findings were folded with
ratchets that reproduce the exact failure; nothing was dismissed as noise. Notable catches the
council forced (all fixed): a cross-principal content oracle; a receipt over-claim; a wrong-target
kill-switch race; an operator-Inbox phishing channel; and a backward-clock replay regression (plus
its persist-failure sub-case) in this very guard.

## Verification status at merge

- `cargo clippy` clean across `elastos-runtime`, `elastos-server`, `elastos-common`.
- Test suites green: **runtime 409 · server 1159 · common 96** (lib), plus the ESP/receipt tools.
- No `todo!()`/`unimplemented!()` in the mandate path; every closed gap carries a ratchet, every open
  gap a documented reason.

## What a reviewer should look at first

1. `elastos-runtime/src/capability/intent.rs` — the gate, the standing-grant store, the replay guard.
2. `elastos-server/src/intent_executor.rs` — the four affordances + the reconciliation seam.
3. `elastos-server/src/api/handlers/capability.rs` — issue/revoke/dispatch handlers + receipt export.
4. `elastos-server/src/api/gateway_mandates.rs` — the shell app's read/grant/revoke surface.
5. `capsules/mandates/index.html` — the operator UI (list, receipt drawer, grant form, kill switch, Agent State).
6. `docs/KNOWN_GAPS.md` — the honest ledger of every gap, closed and open.
