# Spec — fence the three insecure dev modes out of production builds

Date: 2026-06-15. Owner: dDRM team (implement in Cursor). Source: `docs/SECURITY_AUDIT.md`
(2026-06-15 re-audit). This is a *spec*, not a patch — it names exactly what to gate and the
recommended shape. Implementing it closes the residual HIGH and two MEDs at once.

## The problem in one line

The secure dDRM path (signed `AccessGrantV1` + on-chain `hasAccessByContentId` + fail-closed
quorum) is correct. But three **insecure dev/demo conveniences ship enabled by default**, and a
production deploy that runs any of them is exploitable. "Fail closed, then explain" (PRINCIPLES
#11) must extend to *build configuration*: these modes should be **impossible to run in a release
build by construction**, not gated by remembering a flag.

## The three modes to fence (each is a forgeable / free-content path)

| # | Mode | Where | Risk if it runs in prod |
|---|---|---|---|
| 1 | **Reference key backend** (`release_reference`) | `capsules/key-provider/src/main.rs:1238` (recover); only gate is the **unsigned** `validate_rights_receipt_binding` at `:2357`. Default in the open driver: `scripts/dev/ddrm-runtime-open/src/main.rs` (`backend` defaults to `"reference"`) | **HIGH** — forge `allowed:true` in `RightsDecisionReceiptV1` → CEK released. The original finding, live. |
| 2 | **`legacy-receipt-authz` feature** | `capsules/dkms-authority/Cargo.toml:13` (default feature); `reauthorize` at `main.rs:1794`/`:1800` | **MED** — an allow-listed caller with no grant recovers via unsigned-receipt field-compare. |
| 3 | **Dev / ChainMock rights mode** | `elastos/crates/elastos-server/src/api/rights_authority.rs:90` (`rights_mode()` defaults to `Dev`); free buy→ledger→open loop in `buy_authority.rs` | **MED** — free content unlocks (no payment, no signature) if deployed in Dev/ChainMock. |

## Recommended shape — one feature + fail-closed startup guard

**A single `dev-modes` cargo feature** (name it as you like: `unsafe-dev-defaults`,
`insecure-dev`) that gates all three, **off by default**, so a plain `cargo build --release` is
secure by construction; dev/CI/demos opt in explicitly with `--features dev-modes`.

1. **Reference backend** — `#[cfg(feature = "dev-modes")]` on `release_reference` and its
   selectability. Without the feature, the only release path is `dkms` (signed grant). The open
   driver's default `backend` becomes `"dkms"`; `"reference"` is rejected (or unavailable) unless
   the feature is on.
2. **`legacy-receipt-authz`** — remove it from `[features] default = [...]`; keep it as an opt-in
   feature, and only enable it under `dev-modes` (or its own explicit, non-default feature).
3. **Dev/ChainMock rights mode** — `rights_mode()` defaults to **`Chain`** in a non-`dev-modes`
   build; selecting `Dev`/`ChainMock` requires the feature *and* an explicit env var, and the
   runtime **fails closed at startup** if a dev mode is active without `dev-modes` compiled in.

**Belt-and-suspenders runtime guard** (because a feature flag alone can be mis-set): at gateway/
node startup, if any of {reference backend selected, legacy-receipt active, rights_mode != Chain}
is true while `cfg!(feature = "dev-modes")` is false → **panic/exit with a clear message**
("insecure dev mode active in a non-dev build — refusing to start"). Fail closed, then explain.

## Acceptance criteria (how you know it's done)

- A default `cargo build --release` (no `--features dev-modes`):
  - cannot select the reference key backend (or it requires a verified grant);
  - does not compile `legacy-receipt-authz`;
  - `rights_mode()` returns `Chain`, and selecting `Dev`/`ChainMock` refuses to start.
- `cargo build --features dev-modes` restores all three for local/CI use.
- A test asserts the default-release posture: e.g. `release_reference` is `#[cfg]`-gated out, and a
  startup-guard unit test that a dev mode without the feature returns the fail-closed error.
- `docs/SECURITY_AUDIT.md` GAP-7 and the `capability_conformance` harness flip from
  "partially closed" to **closed** once this lands.

## Separately (defense-in-depth, not part of the gate; lower priority)

- **Clamp `effective_now`** (`dkms-authority/main.rs:98`) to the node's real clock — accept caller
  `now_unix` only for issuance/tests, never for security-expiry checks.
- **Wire a process-held `ReplayGuard`** into `authorize_access` (`dkms-authority/main.rs:1768`,
  currently `replay: None`) so per-request-nonce single-use and revoke-by-nonce actually run.
- **Replace `.expect("spawn dkms recover thread")`** (`key-provider/main.rs:1804`) with a counted
  fault so a spawn failure under resource pressure doesn't crash the single-threaded warm daemon.
