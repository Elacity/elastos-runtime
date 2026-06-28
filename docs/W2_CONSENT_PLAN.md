# W2 — unstub the consent ACT path (approval-ready plan)

> **STATUS: ✅ CLOSED (steps 1–11).** Implemented on `claude/keep-consent-architecture-0fz0ll`
> (commits `4833f89` → `025109e` → `c290d65` → `1365acc` → alignment/docs slice). Consent is real,
> cryptographically enforced, single-use + bound, witness-gated at dispatch, blocking-audited, and
> returns a runtime-signed `AffordanceGrantReceiptV1` — with the invariants pinned in
> `check-wci-alignment.sh` against regression. Gates: elastos-runtime capability 128 pass,
> elastos-server lib 854 pass, clippy `-D warnings` + fmt clean, alignment OK.
> **Open follow-up (not blocking):** sign the gateway provider-effect *telemetry* envelope
> (`signer_did: None`); the authoritative attestation is the runtime-signed receipt. The live
> gateway→runtime forwarded-bearer→`vm-{name}` redeem round-trip is integration-verified, not unit-tested.

From planning swarm `wuuc4f5jd` (3 read-only cartographers mapped the live `elastos-runtime` tree at
HEAD `97bcd3689` → architect → 3 adversarial security reviewers, all "needs-fixes", every fix folded
in). PLAN ONLY — per the `elastos-runtime` CLAUDE.md contract, no code until the founder approves.
Implements wedge W2 from `ESP_SHELL_PROTOCOL.md`.

## Goal
Replace the flat HTTP 403 at `enforce_affordance_invocation_policy`
(`elastos/crates/elastos-server/src/api/gateway_capsule_catalog.rs:329-353`) — which dead-rejects every
`AffordanceApprovalMode::User` and high-risk (Payment|Rights|Actuator|Privileged) method — with a REAL
consent round-trip: invoke → 202 + request_id (never a token) → signed consent fact on the existing SSE
inbox → user approve/deny → scoped, +1h-expiring, single-use, revocable ed25519 grant ONLY on approval →
dispatch through ONE canonical validate-and-consume gate → signed request→approve→use audit chain +
a verifiable receipt.

## What the audit found (why this is mostly wiring, with two traps)
A canonical capability request→grant→validate loop EXISTS but is not connected to the affordance act
path, and two structural gaps make naive reuse unsafe:
- **Gap A (seam):** `capsule_interface_invoke` runs on `GatewayState`, which holds no `CapabilityManager`
  and no `PendingRequestStore` (`gateway.rs:214-225`), and the runtime exposes no `validate`/`use` route
  (`server.rs:240-258`). So the gateway can neither validate in-process nor over HTTP today.
- **Gap B (binding):** a pending request carries only `(session_id, resource, action)` and the inbox
  approve verb hardcodes `duration:"session"` → a non-expiring, multi-use token bound to the *session*,
  not the *affordance*. That defeats per-act consent (cross-method + arg-swap replay).

W2 closes both. **The seam decision (load-bearing):** the caller re-presents the granted token to a NEW
authenticated runtime endpoint `POST /api/capability/validate-and-consume` that runs the full 12-check
`validate()` AND atomically consumes the single use *server-side* — NOT ambient server-side correlation
(that's ambient authority), and NOT the gateway trusting a bearer string it cannot cryptographically
verify (that's transport-as-truth). The runtime, which holds the key, is the only validator.

## The consent round-trip (plain terms)
1. Caller invokes a consent-gated method. Gateway maps `resolved.method` → a NARROW `(ResourceId, Action)`
   (evidence-pinned to real manifests), computes `input_hash` (canonical hash of `request.input`), POSTs
   the runtime request endpoint with `principal_id + capsule + interface + method.id + input_hash`, audits
   a SIGNED `capsule.affordance.approval-requested`, and returns **202 AffordanceConsentPending** with the
   store's UUID `request_id` (no token).
2. The pending request raises `capability_request_count` → `inbox.changed` on `/api/apps/home/events/stream`
   → the shell renders the `(capsule, interface, method, resource, action, risk)` fact.
3. User approves → a DISTINCT affordance-consent grant path mints a token bound to `capsule=resolved.capsule`
   (not session_id), with `method_id + input_hash` in `TokenConstraints`, `expiry=now+1h`, `max_uses=1`,
   audits a SIGNED `CapabilityGrant`.
4. Caller re-invokes carrying the token. Gateway POSTs `validate-and-consume`, which runs the 12 checks
   (incl. check-4 `token.capsule==resolved.capsule`), additionally rejects on `method_id` mismatch and
   recomputed `input_hash` mismatch, atomically consumes the single use, audits `CapabilityUse`, returns
   pass + receipt.
5. Gateway dispatches through the ONE `dispatch_capsule_affordance` *only* with a `ValidatedAffordanceGrant`
   witness (a type that cannot be constructed without a successful validate-and-consume — the compiler, not
   call-order, guarantees no dispatch without fresh consent), audits SIGNED `capsule.affordance.completed`,
   returns the receipt. Deny / expiry / forge / replay(2nd use) / revoke / method-swap / arg-swap each fail
   closed with a distinct explicit reason.

## The 11 steps (each gated, smallest-verifiable)
1. **Pin the seam decision** (no code): the correlation key `(principal_id, resolved.capsule, method.id,
   input_hash)` + the new endpoint, citing validate check-4. → founder sign-off.
2. **`affordance_consent_descriptor`** — map each real consent-gated manifest method to its NARROWEST
   evidence-pinned `(ResourceId, Action)` (do NOT blanket-map Payment/Rights/Privileged → Admin); fail
   closed on resource-less methods. → `just check crate=elastos-server` + a mapping unit test.
3. **Binding fields** on `PendingCapabilityRequest` + `RequestCapabilityInput`: `capsule`, `principal_id`,
   `method_id`, `input_hash` (existing callers pass `None`, behavior-neutral). → `just test-crate
   elastos-runtime` + a regression test.
4. **Replace the 403** in `enforce_affordance_invocation_policy`/`capsule_interface_invoke` with
   `request_affordance_consent` → 202 + store UUID. → a test asserting 202 (not 403, no token) + the
   pending list shows the binding fields.
5. **Surface on the inbox SSE** (reuse `capability_request_count` → `inbox.changed`; adjust the
   notification copy). → a stream test observing the consent fact.
6. **Distinct affordance-consent grant path** in `grant_request`: `token.capsule=request.capsule`,
   `method_id+input_hash` in constraints, `expiry=now+1h`, `max_uses=1` — without regressing the existing
   localhost session flow. → grant-shape test + session-flow regression test.
7. **`POST /api/capability/validate-and-consume`** (authenticated): full `validate()` + method/input_hash
   equality + atomic single-use consume + SIGNED `CapabilityUse` + receipt, or a distinct explicit error.
   → a test covering ok / use-limit / expired / method-swap / arg-swap / revoked / forged-sig.
8. **`ValidatedAffordanceGrant` witness** + `require_live_affordance_grant`: `dispatch_capsule_affordance`
   takes the witness for consent-gated methods; no witness → distinct 403, never reaches the handler. → a
   test that absent/expired/revoked/forged/swapped each yields a distinct 403 and the handler is unreached.
9. **Receipt + signed, BLOCKING audit**: `receipt: Option<AffordanceGrantReceiptV1>` on the response; sign
   the affordance envelope (today `signer_did:None`); make the grant→use receipt chain blocking (if the
   signed audit can't be recorded, the act fails closed). → a full-journey test asserting ordered SIGNED
   grant-then-use + an audit-failure-fails-the-act test.
10. **`test_affordance_consent_journey`** mirroring the wallet approval journey + all fail-closed branches
    (deny, replay, expired, revoked, forged, method-swap, arg-swap, cross-session). → `cargo test -p
    elastos-server test_affordance_consent_journey`.
11. **Alignment + docs**: add `check-wci-alignment.sh` assertions (no dispatch arm reachable without the
    witness; no affordance grant without expiry+max_uses=1; validate-and-consume the sole validator for
    affordance tokens); update `state.md`/`TASKS.md`. → `just alignment-check` then `just verify` green.

## Security invariants the code must hold
- FAIL-CLOSED: 202 carries no token; missing/expired/forged/replayed/revoked/method-swapped/arg-swapped
  each yield a distinct explicit 403 from server-side validate-and-consume; no silent downgrade.
- BIND-APPROVAL-TO-AFFORDANCE: token bound to `(principal_id, capsule, method.id, input_hash)`, not merely
  `(resource, action)` and not `session_id`; a grant for methodA fails for methodB sharing the same
  `(resource, action)`.
- BIND-TO-ARGUMENTS: `input_hash` in the consent fact, the token, and re-checked at dispatch;
  approve-then-swap-args fails closed.
- NARROW + EXPIRING + SINGLE-USE: narrowest evidence-pinned action, `expiry=now+1h`, `max_uses=1`, revocable.
- NO-AMBIENT-AUTHORITY: re-invoke MUST carry the token; no path acts on a looked-up session grant; the
  runtime (key holder) is the sole validator.
- ONE-CANONICAL-PATH: validate-and-consume is the single validate+consume point; the witness is the single
  dispatch precondition; the alignment assertion forbids any bypass; 501 `affordance_not_bound` stays the
  honest failure for genuinely unbound methods.
- NON-REPUDIATION: the request→approve→use chain is SIGNED and BLOCKING; ordering grant-then-use asserted.
- CORRELATION ON UUID ONLY: the predictable `capsule-affordance:{...}` string is an audit label, never
  accepted to approve/claim a grant.

Principles honored: P3/P7 (no ambient authority; explicit/narrow/revocable/expiring), P5/P13 (small trusted
core — consent logic in the runtime, prompt+click in the Home/Inbox capsule; net new core = one endpoint +
binding fields + a witness type), P10 (one canonical path), P11 (fail-closed), P2/P9/P16 (identity rooted,
transport an adapter, a UI surface ≠ authority), P12 (docs/code/tests in lockstep).

## Gates
Per step: `just check crate=<crate>`, then `just fmt` + `just lint`; `just test-crate elastos-runtime`
after 3/6/7, `just test-crate elastos-server` after 2/4/5/8/9. Step 10: the journey test. Before done:
`just alignment-check` then `just verify` green — the definition of done.

## Risks / unknowns
- Trusted-core growth: +1 runtime endpoint, +4 binding fields, +1 witness type — the minimum to reach the
  canonical gate; must stay narrow under P5.
- `input_hash` canonicalization (sorted keys, normalized numbers) or legit re-invokes spuriously
  `args_mismatch` — needs a pinned canonical form + a stability test.
- The Risk→Action table is evidence-gated: a consent-gated method with no resource / non-allowlisted scheme
  is permanently deniable (documented fail-closed) — confirm no shipping method is bricked.
- `grant_request`/`GrantRequestInput` is shared with the localhost session flow; the affordance branch must
  be additive + behavior-neutral (regression test).
- 5-min pending TTL + in-memory store: slow/offline shell or a restart expires consent → re-invoke
  (acceptable fail-closed for W2, documented).
- BLOCKING audit changes today's best-effort emit — must not deadlock/unbounded-block; clean explicit
  failure mode.
- Signing the gateway envelope needs a signer the `GatewayState.audit_log` can reach — else the runtime
  co-signs (edges toward the deferred audit-sink unification at `gateway.rs:222-224`).

## Open decisions for the founder (approve before code)
1. **THE SEAM (load-bearing):** caller-holds-token → `POST /api/capability/validate-and-consume`
   (validate+consume+receipt server-side). Approve this over ambient-correlate or gateway-validates-bearer.
2. **Binding fields:** approve adding `capsule/principal_id/method_id/input_hash` to `PendingCapabilityRequest`
   + the grant path (reclassifies `pending.rs`/`capability.rs` from verify-only to MODIFIED).
3. **Grant policy:** `expiry=now+1h`, `max_uses=1`, distinct affordance-consent path. Confirm +1h or specify
   a per-method manifest TTL.
4. **Signed + BLOCKING audit** on the act path, and which key signs the gateway envelope (gateway vs runtime
   co-sign).
5. Confirm all four high-risk classes route through User consent (none is meant to be RuntimePolicy auto-grant).
6. Confirm the evidence-pinned Risk→Action table + the resource-less-method policy (permanent fail-closed).

Source: swarm `wuuc4f5jd`. Next: founder approval → implement steps 1→11, gating each, diffs shown, on
the `elastos-runtime` tree.
