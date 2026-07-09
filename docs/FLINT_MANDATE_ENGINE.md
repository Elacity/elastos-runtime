# Flint — The Mandate Engine

**"Give your agent a mandate, not your keys."**

Flint is the mandate/payment engine inside the ElastOS runtime: the layer that lets an operator
hand an AI agent a *scoped, expiring, revocable* mandate instead of raw credentials, have the
agent act under it unsupervised — including spending real money under a provable cap — and hold
the whole thing to a tamper-evident, off-box-verifiable record. This document is the reviewable
map; the detailed per-gap ledger lives in `KNOWN_GAPS.md`.

**Vocabulary.** A *mandate* is a standing grant — a signed capability token plus its authorized
method envelope. The API calls it a standing grant (`/api/standing-grants/dispatch`), the audit
chain records it as `CapabilityGrant`/`CapabilityUse`, and the operator UI calls it a mandate;
all three name the same object.

## The lifecycle, in one place

| Verb | What it is | Where |
|---|---|---|
| **Grant** | Mint a real signed capability token scoped to (capsule, resource, action, ttl) + an authorized method set + the **responsible entity** (the DID the OPERATOR DECLARES accountable for the agent's acts — the EU-AI-Act liability *attribution*, signed into the grant record and the receipt; required on the shell app). **Operator-asserted, not attested:** the runtime records the DID verbatim; it does not resolve it or obtain the entity's counter-signature, so a receipt proves the operator's declaration, not the entity's consent (entity counter-signing is a tracked gap). | CLI + Mandates shell app |
| **Act** | An agent signs an `IntentDeclarationV1` and dispatches it; a fail-closed gate checks `intent ⊆ envelope` (capsule + agent-key + method + resource + action), runs a real executor, and reconciles declared-vs-done | `POST /api/agent/dispatch` (agent-facing, S26) or `/api/standing-grants/dispatch` (operator) |
| **Watch** | The operator sees mandates live, what each agent has written/delivered under them, and — for pay-mandates — the marketplace assets they scope (live on-chain quotes) and the buys as the ledger records them (pending always shown; settled ones windowed, stated) | Mandates shell app (mandate cards, Agent State panel, Inbox, Money + Marketplace panels) |
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
| `runtime.market_quote` | read (side-effect-free) | `read` | The agent SHOPS within its mandate (S39): quote the live on-chain terms of a granted asset (`elastos://runtime/pay/<asset>` — no market-wide oracle), through the ONE single-flight TTL-cached quote spine the Marketplace panel shares. One envelope carries ONE action, so quoting takes a `read` mandate on the same pay resource the `execute` pay-mandate scopes (two grants, one resource). Discovery mode (empty `input_hash`) performs and returns the terms via the response's explicit-disclosure channel; attested mode (declared terms) `Matched`-attests the terms AS OF THE SPINE'S LAST READ (≤30s old — a change inside the cache window is caught on the next re-read) and a changed listing reconciles `diverged`. A failed read declines honestly — `performed` only when terms truly returned (dev/chain-mock modes return synthetic free terms, as the panel also states) |
| `runtime.negotiate` | propose (non-value-moving) | `read` | The shop loop's MIDDLE leg (S50 — quote → **negotiate** → pay): the agent makes a bounded OFFER for a granted asset (same `elastos://runtime/pay/<asset>` scope), and the injected seller answers accept / counter / reject. THE PROVABLE PROPERTY: the offer rides the signed `input_hash` as a canonical positive integer of spend units, and an offer above the mandate's UN-SPENT cap (`SpendMeter::remaining`) is refused BEFORE it reaches the seller — an agent can never PROPOSE to commit its operator beyond granted authority. It only READS the cap (no reserve, no debit, no broadcast), so NO money moves here; settlement stays `runtime.pay`. Like `market_quote` it is a READ-authority probe (the dispatch gate authorizes only the fixed Action enum; the OFFER, not the action, makes it a proposal) — so it takes a `read` mandate on the same pay resource the `execute` pay-mandate scopes. Accept/counter `performed` echoing the offer (the receipt attests the bounded offer, NOT the counterparty's disposition, which the runtime cannot verify); the seller's terms ride the response's disclosure channel (ephemeral market data). Rejection declines honestly. Wired only where a rail has a listing to negotiate against (the DRM marketplace's fixed-price seller reuses the buy gate's EXACT spend-unit conversion + pay-token guard); unwired on HTTP/ERC-20 |
| `runtime.notify` | **side-effecting** | `message` | Delivers a message into the operator's Inbox; bounded fields, capped store, `performed` only after the write lands |
| `runtime.state_put` | **side-effecting** | `write` | Writes durable, readable-back, principal-scoped agent state; last-write-wins with attributed versioning |
| `runtime.pay` | **side-effecting, money** | `execute` | Spends real money to a mandate-scoped payee under a durable spend cap: the amount rides in the signed `input_hash`, over-cap or unprovisioned refuses with no money moved, and every outcome is classified two-generals-honestly. The full design — durability, rails, custody, reconciliation, the operator surfaces, and every honest bound — is in [The payment spine](#the-payment-spine-runtimepay) below |

## The payment spine (runtime.pay)

One pay spine, never a fork: whatever rail is wired, the meter, the ledger, the outcome
classification, and the signed receipt are byte-identical.

### The cap is enforced, durable, and operator-provisioned

- The spend meter reserves against the per-capsule cap ATOMICALLY before any money moves; over the
  cap (or an unprovisioned capsule ⇒ zero) the payment is REFUSED with a signed
  `authorized_not_performed` — no money moved (S27).
- The cap is DURABLE (snapshot + fsync; the reservation persists BEFORE money moves; a restart
  never refills it; a corrupt or tampered-shape snapshot refuses to boot) and provisioned at
  `POST /api/spend-budgets` or the Mandates Money panel. Each provision is attested on the signed
  chain as a `ConfigChange` and rolled back if the attestation fails (S28).
- The meter POISONS on a post-publish persist failure (mutations refuse until reopened from disk;
  memory never diverges from the visible snapshot) and holds a single-opener flock.

### Outcomes are classified two-generals-honestly

- **Charged** — the rail confirmed it; the spend stands and the rail reference is custodied.
- **Provably not charged** — refund the reservation.
- **Indeterminate** (timeout / 5xx / panic / broadcast-unconfirmed) — KEEP the reservation:
  refunding against money that may have moved would break the cap, the one unbreakable invariant.
  The decline reason names the idempotency key for rail-side reconciliation, and
  `authorized_not_performed` here means NOT-ATTESTED, not proven-absent.

### The rails (one trait, two implementations)

- **HTTP rail (S29):** `ELASTOS_PAYMENT_ENDPOINT` (+ optional bearer `ELASTOS_PAYMENT_TOKEN`)
  wires `HttpPaymentProvider` — a payment order (`payee`, `amount`, signature-derived
  `Idempotency-Key`) POSTed to the deployment's payment service (a thin adapter fronts
  Stripe/ACH/treasury/a crypto rail). HTTPS enforced (plaintext only to loopback), malformed
  endpoints refuse at boot, redirects are never followed (a 3xx is indeterminate, never
  "charged"). Requires the durable meter. The endpoint's obligations are the stated contract in
  `docs/PAYMENT_ENDPOINT_CONTRACT.md`.
- **DRM marketplace rail (S34–S36):** `ELASTOS_PAYMENT_RAIL=drm` settles a buy ON-CHAIN via the
  Elacity `buy_authority` path behind the SAME `PaymentProvider` trait — resolve (fail-closed on
  ambiguity), read-only price quote, the price gate (`amount × ELASTOS_DRM_SPEND_UNIT` must cover
  the on-chain price, quantity pinned to 1, pay-token declared and drift-armed), broadcast ⇒
  PENDING, and promotion to charged only after the tx is mined + successful + past the
  confirmation-depth floor — driven unattended by the in-runtime confirmation scheduler when
  `ELASTOS_DRM_RECONCILE_INTERVAL_SECS` is set (S37). The full rail — wiring, runbook, honest
  bounds — is `docs/DRM_MARKETPLACE_RAIL.md`.
- The Mock rail stays dev/demo-gated behind `ELASTOS_ALLOW_MOCK_PAYMENTS` (a real endpoint wins if
  both are set).

### Custody and reconciliation (S30/S35)

- The payment LEDGER durably custodies every rail attempt the process lived to record — the
  performed payment's rail reference and a PENDING entry per indeterminate outcome (per-capsule
  bounded, so one agent cannot blind a victim's obligations). Money-bearing keys are NEVER
  evicted, and the durable ledger is the cross-window idempotency: a re-dispatched signed intent
  whose key already moved-or-may-have-moved money is refused, never re-charged.
- Pending entries surface at `GET /api/payments/pending` and resolve EXACTLY ONCE at
  `POST /api/payments/reconcile` (`charged=false` refunds, `charged=true` confirms; each
  resolution attested on the signed chain). DRM pendings are additionally resolvable against the
  chain itself via `reconcile_drm_confirmations` (see the DRM doc).

### The operator surface and its authorization perimeter (S31/S33)

- The Money panel (budgets with cap/spent/remaining/held-unconfirmed, the poisoned banner, the
  reconciliation work list) is a read-only projection of the ONE enforcing meter+ledger
  (`build_pay_rail`, same-Arc by construction, flock-enforced).
- The Marketplace panel (S38) is the same discipline pointed at the marketplace: the assets the
  ACTIVE pay-mandates scope (with live on-chain quotes — TTL-cached and fan-out-bounded, so a
  browser refresh storm is not a chain-read storm — one live read per asset per window, enforced
  single-flight) and the buys as the ledger records them — every PENDING buy always (a flood of
  new settled entries can never push a live obligation out of sight), the settled tail windowed
  with the window stated — worded honestly (a broadcast is "awaiting chain confirmation", never a
  purchase). STRICTLY read-only:
  the panel has no buy verb — operators grant, agents act through the signed-intent dispatch.
  One-command walkthrough: `elastos mandate market-demo <asset>` (cap → pay-mandate → the
  agent's signed buy → watch it resolve in the panel).
- Web provisioning is CEILING-BOUND server-side (`ELASTOS_WEB_MAX_SPEND_CAP`); verdict buttons are
  arm→confirm; a not-charged verdict is refused while the meter is poisoned (it would burn the
  one-shot refund handle).
- The mandates launch token is an HttpOnly, SameSite=Strict cookie path-scoped to the mandates API;
  cookie-authorized writes require the anti-CSRF app-marker header; and every money write requires
  a FRESH passkey verification (a WebAuthn ceremony ≤180s old, proof-bound to the same principal)
  SPENT on exactly one applied write.

### Honest bounds, stated

- The cap is PER-CAPSULE, not per-mandate; an orphaned/indeterminate reservation over-counts the
  cap fail-closed (recovery: rail-side lookup by idempotency key FIRST, then a deliberate cap
  raise).
- A crash between the persisted reservation and the rail verdict leaves a durable reservation with
  no ledger entry (recovered from the on-chain declaration); pending custody is
  guaranteed-or-stated, terminal records best-effort.
- The 4xx⇒not-charged rule is a stated CONTRACT on the payment endpoint; a lying 2xx from a
  compromised endpoint mints receipts the runtime cannot independently check — an HTTP-rail
  Performed pay is a RAIL-TRUST attestation, weaker than the runtime-verified affordances. (A DRM
  buy is stronger: it is chain-CONFIRMED before it is charged.)
- The spent-passkey guard is in-memory (a gateway restart inside the ~3-minute window could admit
  one replay), and a runtime with no passkey enrolled makes money writes CLI-only — both tracked
  as G-M9.
- The snapshot files are trusted from `data_dir` (not self-authenticating, unlike the signed
  chain); the flock/parent-fsync protections are unix-only. Every provider subprocess a
  money/access path traverses (chain, wallet-sign, rights-decide) AND every access-path content
  sidecar (the media/object authorities and the grant sidecar, S42) is deadline-bounded (S40–S42,
  one shared watchdog) — no runtime thread parks forever on a hung or hostile provider, and the
  bounded reap leaves no zombie; the kill is unix-only.
- DRM-rail residuals are tracked as `MKT-DRM` in `KNOWN_GAPS.md` (the operator-declared spend-unit
  mapping; the confirmation scheduler being opt-in — unset interval ⇒ back to the manual
  reconcile loop).

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

Every increment ran the same standard before commit: gate (build + clippy + full test suites),
then two independent adversarial reviews (a principles guardian and a red team), then every
finding folded with a ratchet test that reproduces the exact failure — nothing dismissed as noise.

## Verification status

- `cargo clippy` clean across `elastos-runtime`, `elastos-server`, `elastos-common`; test suites
  green (run `cd elastos && cargo test --workspace`, or `just test`).
- No `todo!()`/`unimplemented!()` in the mandate path; every closed gap carries a ratchet, every open
  gap a documented reason.

## What a reviewer should look at first

1. `elastos-runtime/src/capability/intent.rs` — the gate, the standing-grant store, the replay guard.
2. `elastos-server/src/intent_executor.rs` — the affordances (including `runtime.pay`) + the reconciliation seam.
3. `elastos-server/src/api/handlers/capability.rs` — issue/revoke/dispatch handlers + receipt export.
4. `elastos-server/src/api/gateway_mandates.rs` — the shell app's read/grant/revoke surface.
5. `capsules/mandates/index.html` — the operator UI (list, receipt drawer, grant form, kill switch, Agent State).
6. `docs/KNOWN_GAPS.md` — the honest ledger of every gap, closed and open.
