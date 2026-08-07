# Capability conformance audit

Date: 2026-06-14. Method: four read-only sub-agents, one mapping the enforcement
architecture and three inventorying privileged effects by class (keys/signing,
network/chain/filesystem, launch/root-data/audit). Every claim is `file:line`, read not
grepped. The machine-checked half lives in
`elastos/crates/elastos-runtime/tests/capability_conformance.rs` — the `KNOWN_GAPS`
registry there mirrors this document's findings and runs under `just verify`.

This audit answers one question: **is the runtime's central invariant — no privileged
effect without a valid, scoped capability token — true in code, or only designed?**

## Architecture: no single chokepoint, two authority tiers

There is **no single gate**. Enforcement is distributed across **6 call sites** of
`CapabilityManager::validate`, and the provider registry where privileged effects funnel
(`elastos/crates/elastos-runtime/src/provider/registry.rs:596 route`, `:724 send_raw`)
performs **no check** — it trusts callers. So "nothing is ambient" holds *by convention
at each call site*, not *by construction*.

The 6 enforcement points: `handler/request_handler.rs:742` (storage/resource),
`messaging/channel.rs:232` (capsule→capsule), `carrier_bridge.rs:686` (guest/remote
invoke, incl. wallet sign), and the HTTP handlers `api/handlers/provider.rs:115`,
`namespace.rs:82`, `storage.rs:453`. Caller identity (`from`) is transport-bound, not
self-asserted in the request body, and `validate` check #4 binds the token to it — so the
model is sound *as long as every privileged path remembers to call validate and `from`
cannot be spoofed*.

Two authority worlds are held to different standards:
- **Gateway / launch layer** (`elastos-server` + `elastos-auth`): strong — v2 launch
  tokens are gateway-DID-signed, bound to principal+session+grant+expiry+`non_delegatable`,
  with active-session checks.
- **Runtime-core in-VM layer** (`elastos-runtime` capability/shell): weaker — it predates
  the principal contract (see GAP-1, GAP-3, GAP-8).

## What is proven strong

- **The WASM boundary holds.** `build_wasi_context`
  (`elastos/crates/elastos-compute/src/providers/wasm.rs:307`) gives a capsule only
  stdio + env + ≤2 preopened dirs — no `inherit_network`, no host mount. An app capsule has
  **no ambient filesystem and no network**; it reaches providers only through the
  `/_carrier` FIFO bridge carrying a capability token. The core "no ambient authority"
  claim is **structurally enforced** for app capsules. This is the most important result.
- **The dDRM key paths are excellent.** `decrypt-provider`/`key-provider`/`dkms-authority`
  gate CEK release on transcript-bound AES-GCM AAD + ML-DSA-65 signatures + non-replayable
  session tokens + escrow recovery — they do not trust `principal_id`.
- Network/chain/FS providers are uniformly fail-closed with config-allowlists and
  `#[serde(deny_unknown_fields)]`; TAP/TUN creation is manifest-gated to `role=provider`
  (`elastos-common/src/manifest.rs:257`); no capsule self-mint/refresh path exists;
  delegation is depth-1, scope-narrowing-only.
- The capability validator itself denies wrong-capsule / wrong-action / wrong-resource /
  expired / over-use-limit / revoked tokens, and tokens are unforgeable by external code
  (fields are `pub(crate)`). These are the passing probes in the harness.

## Findings (the conformance debt) — see `KNOWN_GAPS` in the harness

> **Reconciled 2026-06-14 by [SECURITY_AUDIT.md](SECURITY_AUDIT.md):** GAP-1 and GAP-4 were resolved
> as **SAFE** (identity is transport/host-bound; principal_id derives only from a gateway-signed
> launch grant). GAP-2 and GAP-3 are downgraded to **low** (reachable only by the host-bound shell,
> not untrusted capsules). GAP-7 is **confirmed High** with a concrete exploit (and is being fixed
> on the dDRM side). The table below is the original inventory; `KNOWN_GAPS` in the harness carries
> the reconciled severities.

| ID | Sev | Finding | Location |
|----|-----|---------|----------|
| GAP-1 | high | Shell exemption: handlers short-circuit validation when `caller == shell_id`; soundness rests on `from` being unspoofable and a single shell | `request_handler.rs:646/553/588`, `messaging/channel.rs:227`, `api/handlers/provider.rs:78` |
| GAP-2 | high | Provider registry `route`/`send_raw` do no capability check; by-convention only | `provider/registry.rs:596,:724` |
| GAP-3 | high | Runtime-core `grant` mints a signed token from caller-supplied strings, gated only by shell identity; `CapabilityToken` has no principal/proof/device fields | `capability/manager.rs:237`, `token.rs:193` |
| GAP-4 | high | `principal_id` is self-asserted inside the capsule boundary; fail-closed only if `carrier_bridge` binds it to the session first | `carrier_bridge.rs:686` |
| GAP-5 | high | `export_managed_secret` exports a raw private key gated only by self-asserted `principal_id`, with no audit | `capsules/wallet-provider/src/account.rs:424` |
| GAP-6 | med | DID signing has no authorization gate (any caller with `sender_id`+`ts`) | `capsules/did-provider/src/main.rs:281/293` |
| GAP-7 | med | Rights-decision receipts are unsigned yet drive CEK-release authz; saved only by escrow crypto | `elastos-common/src/protected_content.rs:228`, `key-provider/src/main.rs:2122` |
| GAP-8 | med | Runtime-core audit sink is best-effort and unsigned; a denial whose write fails is silently dropped | `primitives/audit.rs:300` |
| GAP-9 | med | Launch tokens lack device binding, and `proof_binding_id` is `Option` — a token without it skips the active-session check | `gateway_home_token.rs:339` |
| GAP-10 | low | Human-vs-AI is a naming convention, not enforced: `validate` never branches on `Users/` vs `UsersAI/`; principals hardwired to `Users/` | `manager.rs:328`, `auth.rs:1175` |

Already tracked in `TASKS.md §1`: GAP-9, the `Users/self` cleanup, principal-root encryption
coverage. **Untracked, surfaced here:** GAP-1, GAP-2, GAP-3, GAP-8.

## The ratchet

The harness is wired into `just verify` (it's a workspace test). The passing probes lock in
the invariants that hold. Each gap is an `#[ignore]`d placeholder (so it does not break the
gate for in-flight dDRM work) plus a `KNOWN_GAPS` row. **To close a gap:** implement the
fix, replace the `#[ignore]`d placeholder with a real assertion, and delete the row. Once
dDRM lands, flip the high-severity placeholders to blocking (remove `#[ignore]`). That
converts this inventory from a document into a build-enforced contract.
