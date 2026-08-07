# Roadmap Coverage Report — `feat/dkms-esp-port` vs `upstream/0.7-dev` Roadmap

**Date:** 2026-08-07
**Branch:** `feat/dkms-esp-port` @ `e2cc4229` (5 commits, READY-FOR-MANUAL-MERGE)
**Method:** every claim below is grounded in code/tests landed on the branch. Two lenses per item:
- **Functional %** — how much of the item's scope works today.
- **Aligned %** — the same scope discounted for refactoring still owed to the target ESP model (facts/verbs conversation plane, Runtime-owned authority, capsule interaction contracts). Where the merge already forced the refactor, the two are equal.

---

## Summary

| Roadmap section               | Functional | Aligned  | Verdict                                                                                     |
| ----------------------------- | ---------- | -------- | ------------------------------------------------------------------------------------------- |
| Foundation                    | **~15%**   | **~12%** | Branch consumes the foundation; only invoke-path security and commerce-slice UI contributed |
| Content creation and playback | **~75%**   | **~70%** | **This branch is this section** — 3 of 4 items at 65–85% functional                         |
| Hardening                     | **~20%**   | **~20%** | Real pieces landed, no item complete                                                        |
| Later follow-ups              | **~15%**   | **~15%** | Precursors only, except egress-policy (~45%, pre-seeded)                                    |

---

## ESP-model alignment: what's on-model vs refactor backlog

The percentages only subtract alignment debt where noted — this section is the explicit accounting.

**Already on the target model (merge did the refactor; no hidden debt):**
- **Provider plane** — component model + hostcall capability ceilings, `provider_operation_action` with G3b preview==enforce, first-writer-wins pinning, identity-keyed teardown, microvm `authority` blocks. Inter-*provider* interaction is ESP-native.
- **Manifests/catalog** — ESP projection schema (`runtime_abi`, `execution`, `projections`), typed `RuntimeCapsuleAffordanceBinding` dispatch.
- **Viewers/Creator as web-projections** — gateway HTTP + fragment launch tokens *is* the sanctioned ESP projection pattern for browser capsules.
- **Money-verb authority** — Home-cookie + intent-bound step-up + Home-chrome spend confirmation; no new authority path.

**Functional but NOT yet on the target model (the refactor backlog):**
1. **No ESP facts/verbs for commerce/viewer surfaces** — the rails speak REST-ish gateway routes (`/api/market/buy`, `/api/viewers/open`) and the marketplace↔Home bridge is a bespoke 5-key postMessage shape, not typed `elastos.esp.*/v1` fact/verb schemas. *Context: main's own wallet/system surfaces are in the same state — this debt is shared and roadmap-forward (`feat/shell-ui-esp` targets it), not a dkms-specific regression.*
2. **Subject-resolution rewire** (`RequiredHomeLaunchToken` threading) — the alignment refactor routing identity through Runtime-owned launch context; currently parked, chain-gated open/buy is dev-lane-only.
3. **Split auth posture on `/api/market/*`** — buy is Home-token-only while sibling routes accept capsule tokens; converges when the verbs migrate.
4. **Two freshness shapes** — Creator guarded at wallet-approval, dkms verbs at step-up (adjudicated acceptable; still model divergence).
5. **`walletPersonalSign`** still on direct `window.ethereum` — aligned fix is delegation challenges (mandate-core), deferred with documented rationale.
6. **dkms's typed method input/output schemas** dropped during manifest migration rather than proposed into main's convention.

---

## 1. Foundation — ~15% functional / ~12% aligned

| Item                                  | Func | Aligned | Covered                                                                                                                                                                                                                                                                                          | Not covered                                                                                            |
| ------------------------------------- | ---- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------ |
| `feat/shell-ui-esp`                   | ~30% | **~15%** | Commerce slice of the shared UI: owned-asset open flow, staged loading, Home spend-confirmation dialog, wallet-metamask polish, Library viewers. Honors the item's core constraint — money verbs brokered under Home's existing authority, **no new authority path created**                     | The general shared desktop/application UI program; **commerce UI runs on bespoke postMessage + REST, not ESP facts/verbs** (backlog item 1) |
| `feat/elastos-carrier-security`       | ~35% | ~35%    | Invoke path authority-gated: op→action enforcement before dispatch (`carrier_bridge`), G-ID (`vm_id`) fail-closed caller identity, refused carrier claims fail closed, identity-keyed teardown                                                                                                   | Carrier as authenticated/bounded/lifecycle-owned _transport_ (budgets, framing-level auth)             |
| `feat/elastos-carrier-protocol`       | 0%   | 0%      | —                                                                                                                                                                                                                                                                                                | Signed, versioned, replay-resistant Runtime-to-Runtime protocol                                        |
| `feat/elastos-collaboration-provider` | ~0%  | ~0%     | Incidental chat touches only (manifest take-main, stderr logging)                                                                                                                                                                                                                                | Typed message/room/collaboration contracts                                                             |
| `feat/elastos-content-availability`   | ~25% | ~25%    | Rail _exercises_ fetch/pin/availability: acquire pins into Library, `content_index`, availability-provider updates, `availability`/`peer` sub-routes pinned                                                                                                                                      | The **one unified** fetch/pin/provide/cache/repair contract — consumers still speak their own dialects |
| `feat/elastos-carrier-core`           | 0%   | 0%      | —                                                                                                                                                                                                                                                                                                | Depends on the two extractions above; not started                                                      |

## 2. Content creation and playback — ~75% functional / ~70% aligned (the branch's home section)

| Item                             | Func | Aligned | Covered                                                                                                                                                                                                          | Not covered                                                                                                                                                                          |
| -------------------------------- | ---- | ------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `feat/elastos-webspace-interop`  | ~10% | ~10%    | Enabling fix only: `webspace-provider` re-bound to the pinned `webspace` slot via `localhost_delegated_scheme` (was routeless)                                                                                   | Cloud/IPFS/DID/friend-space mounting through replaceable providers                                                                                                                   |
| `feat/elastos-dkms-custody`      | ~65% | ~65%    | Threshold custody core: `dkms-authority`/`dkms-keygen`, PQ-hybrid threshold envelopes, key escrow rail, release fail-closed invariant (`legacy-receipt-authz` → `compile_error!`) now CI-gated, audit chains. Provider-plane work — already on-model | The "re-prove" half: degraded recovery, fault-tolerance proofs, node-identity lifecycle, operator evidence surfaces                                                                  |
| `feat/elastos-protected-content` | ~85% | **~75%** | Rights/key/encrypt/decrypt/drm providers, CENC core, media packaging, both viewers (`elacity-player`, `ddrm-viewer`), session lifecycle (open/close/sweep) with e2e anchor proving the full contract fail-closed | Subject-resolution rewire (chain-gated open/buy is **dev-lane-only** until `RequiredHomeLaunchToken` threading — backlog item 2); real decrypt-binary chain runs in no CI lane (boundary-tested only) |
| `feat/elastos-content-commerce`  | ~85% | **~65%** | Full publish→index→buy→acquire rail; Wallet integration (step-up, approvals); Chain integration (`chain_tx`, chain-provider); content-market + marketplace capsules; e2e-anchored                                | `/api/market/search` untested over HTTP; Create-portal browser caller forward-compat only; discover/UX depth thin; **verb surface + market-route auth split await ESP migration** (backlog items 1, 3) |

## 3. Hardening — ~20% (functional ≈ aligned; work landed here is on-model)

| Item                              | Func | Aligned | Covered                                                                                                                                                                              | Not covered                                                                                 |
| --------------------------------- | ---- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| `fix/component-runtime-hardening` | ~25% | ~25%    | Manifest-capability ceilings enforced fail-closed in component hostcall; denial classification (refusals reach guests as `Denied`, not `Internal`); session bounds; capsule watchdog | Per-activation memory/fuel/deadline/instance limits — the resource-bounding core            |
| `feat/elastos-capsule-trust`      | ~20% | ~20%    | `resolve_verified_signer` (honest-`None`, deliberately not an enforcement gate), capability receipts, integrity-verified manifest assumption                                         | Full signed-bundle / publisher / dependency-closure / cross-node receipt verification       |
| `feat/elastos-runtime-lifecycle`  | ~30% | ~30%    | Identity-keyed teardown, viewer-session sweeper, boot-bind hard-fail, truthful refusal statuses                                                                                      | Restart/cancellation program; known gap: refused carrier claim leaves an unreapable capsule |
| `feat/elastos-remote-access`      | 0%   | 0%      | —                                                                                                                                                                                    | ela.city domains, DNS/TLS, tunnel lifecycle, passkey-origin migration                       |

## 4. Later follow-ups — ~15%

| Item                             | Func     | Aligned | Covered                                                                                                                                                                                                                         | Not covered                                                                                         |
| -------------------------------- | -------- | ------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `feat/elastos-mandate-core`      | ~15%     | ~15%    | Step-up tokens already signed, scoped, expiring, intent-bound, single-use (narrow mandate precursor); `access_grant.rs`; `walletPersonalSign` deferral explicitly points at delegation challenges (this item) as the proper fix | The general delegated-authority model (agent-bound, revocable)                                      |
| `feat/elastos-agent-budget`      | ~5%      | ~5%     | Single-use grant refund semantics (`NoProvider`/`DidNotAct`) as a distant cousin                                                                                                                                                | Reserve/commit/release/receipt accounting                                                           |
| `feat/elastos-egress-policy`     | **~45%** | ~45%    | **The sleeper:** `egress_audit.rs` + `egress_firewall.rs` in crosvm, `c4_egress_spine` test suite, `net`/`exit` sub-routes pinned — Runtime-controlled default-deny with audited decisions substantially seeded                 | Policy surface/configurability; formal default-deny rollout for all microVMs                        |
| `feat/elastos-capsule-inspector` | ~10%     | ~10%    | Inspector docs reconciled to live self-tier routing                                                                                                                                                                             | The read-only ESP-facts renderer itself (dkms-era affordance tests were dropped — shipped on Flint) |

---

## Key takeaways

1. **This branch ≈ the "Content creation and playback" section**: custody ~65%, protected content ~85% (75% aligned), commerce ~85% (65% aligned); webspace-interop (~10%) is the section's remaining workstream.
2. **The data and authority planes are on the new model; the conversation plane is not.** How shells and app capsules talk to the rails (REST + bespoke postMessage instead of ESP facts/verbs) is the main alignment refactor — and it's the same migration main's own wallet/system surfaces still owe, so it belongs to `feat/shell-ui-esp`, not to a dkms rework.
3. **Biggest in-section gap:** the subject-resolution rewire (`RequiredHomeLaunchToken` threading) — the one item between "dev-lane-proven" and "chain-mode-live", and itself an alignment refactor. Prerequisites documented in the `viewer_open.rs` PARKED block and `docs/dkms/SECURITY_MODEL.md`.
4. **Foundation and Hardening items were touched only where the rails forced it** — invoke-path security, ceilings, teardown — none is a completed roadmap item.
5. **Scheduling notes:** `feat/elastos-egress-policy` is ~45% pre-seeded — consider pulling it earlier or scoping it down. When `feat/shell-ui-esp` lands the verb migration, revisit commerce/protected-content: their aligned % converges up to their functional % with little dkms-side work.
