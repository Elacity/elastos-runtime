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
| **Grant** | Mint a real signed capability token scoped to (capsule, resource, action, ttl) + an authorized method set, elevated to a standing mandate | CLI + Mandates shell app |
| **Act** | An agent signs an `IntentDeclarationV1` and dispatches it; a fail-closed gate checks `intent ⊆ envelope` (capsule + agent-key + method + resource + action), runs a real executor, and reconciles declared-vs-done | `POST /api/agent/dispatch` (agent-facing, S26) or `/api/standing-grants/dispatch` (operator) |
| **Watch** | The operator sees mandates live, and what each agent has written/delivered under them | Mandates shell app (mandate cards, Agent State panel, Inbox) |
| **Revoke** | The kill switch — durably attested *before* the mandate dies | CLI + Mandates shell app |
| **Prove** | Export a portable `MandateReceipt` and verify it off-box with no runtime and no trust in this box | `elastos verify-receipt` |

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
| `runtime.pay` | **side-effecting, money** | `execute` | Spends real money to a mandate-scoped payee, capped by the spend meter (S27). The amount rides in the signed `input_hash` (canonical decimal); over the cap (or an unprovisioned capsule) the payment is REFUSED with no money moved (a signed `authorized_not_performed`); a rail failure/panic REFUNDS the reservation. Rail-agnostic (`PaymentProvider` — card/ACH/Stripe); **cryptography, not cryptocurrency**. Opt-in via `with_payments`. **Honest bounds (council):** dev/demo-gated (`ELASTOS_ALLOW_MOCK_PAYMENTS`) — the Mock rail must not ship to production; the cap is PER-CAPSULE not per-mandate; the meter is IN-MEMORY (not durable across restart). A DURABLE cap + a real rail connector + an operator provisioning surface are the hard prerequisites before real money moves (S28) |

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
