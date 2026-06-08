# PC2 → Runtime convergence: v0.4.0 working plan + Anders coordination

> Draft (uncommitted). Working plan for the week of 2026-06-08 and a
> paste-ready note to send Anders once the GitHub account is reinstated (or
> via another channel sooner). Grounded in `docs/PC2_CONVERGENCE.md`.

## Context

- Anders' `0.4.0` branch (off `main`/0.3.0) ships: **app marketplace**
  (= PC2 `app-center`), **object-provider file manager** (= PC2 FileExplorer →
  `library` capsule), and **provider invocation transfer rails**
  (`provider/registry.rs`, `gateway_provider_proxy.rs`, `server_infra.rs`).
- This week Anders is doing **Settings** (→ `system` capsule).
- Mac VZ work (`sash/local-test-v030`) is a **separate platform decision** —
  Anders' own `TASKS.md` lists "review Sash's macOS VZ / `elastos-crosvm`
  Darwin substrate branch as a separate platform decision." It stays a parked,
  standalone review branch; no new feature work is layered on it.

## Branch topology (all local; nothing pushed yet)

| Branch | Base | Purpose |
|---|---|---|
| `sash/local-test-v030` | 0.3.0 (`8acb72d`) | Mac VZ platform deliverable. **Frozen** as the standalone review branch (PR #3 when pushable). |
| `sash/v040-integration` | `origin/0.4.0` | "Current engine" base. Re-synced via `git fetch && git rebase origin/0.4.0` as Anders pushes. |
| `feat/decrypt-provider-cenc` | `origin/0.4.0` | First convergence port (this week's primary work). |
| `chore/bincode-2-migration` | `origin/0.4.0` | Security quick-win: `bincode 1.3.x → 2.x` + serialization compat tests. |

Staying current with Anders (no push needed — `git fetch` reads the public repo):

```bash
git fetch origin
git checkout sash/v040-integration && git rebase origin/0.4.0
git checkout feat/decrypt-provider-cenc && git rebase sash/v040-integration
```

## Division of labor (no overlap with Anders)

**Ours this week (provider plane + security):**
- `feat/decrypt-provider-cenc` — port PC2's `cenc-decrypt` Rust crate
  (`pc2-node/crates/cenc-decrypt`, ~1.3k LOC) as the backend of the existing
  `capsules/decrypt-provider` scaffold. CENC (Common Encryption) decrypt is the
  foundational primitive the whole protected-content chain
  (`decrypt → rights → key → drm`) builds on, so it unblocks the most.
- `chore/bincode-2-migration` — contained dependency hardening Anders flagged
  as explicit 0.3.1 follow-up debt.

**Explicitly NOT touching (his lane / moving targets):**
- `library` (FileExplorer) — shipped in 0.4.0.
- `system` (Settings) — his this week.
- `marketplace` / `app-center` — shipped in 0.4.0.
- `provider/registry.rs`, `gateway_provider_proxy.rs`, `server_infra.rs` — his
  transfer rails. We **consume** these, we don't edit them.
- `compute/providers/wasm.rs` / FIFO transport — converged independently; defer
  to his canonical implementation during rebase.

## Convergence guardrails (from `docs/PC2_CONVERGENCE.md`)

Before the port lands, it must name:
- the runtime principal/session/capability it depends on,
- the provider capsule that owns the dangerous authority (here: `decrypt-provider`),
- the app-visible API that stays protocol-agnostic (apps get decrypted output, **never** CEKs/keys),
- the fail-closed test proving apps can't bypass the provider plane,
- the PC2 source commit used as reference.

## End-of-week push plan

- Per-feature topic branches → PRs into `0.4.0` (the active release line).
- Mac VZ stays its own PR as a platform decision (does not block the convergence PRs).

---

## Paste-ready note for Anders

> **Subject:** This week's plan — PC2 convergence (provider plane), staying clear of your FileExplorer/Settings lane

Hi Anders — quick alignment so we run in parallel and connect cleanly for the next release.

**Mac VZ:** staying exactly as you called it — a parked, standalone platform-review branch (your `TASKS.md` line about reviewing it as a separate platform decision). I'm not layering anything new on it.

**This week I'm basing new work on your `0.4.0`** (tracking it locally and rebasing as you push) so everything integrates against the current engine — your transfer rails, FIFO transport, and marketplace catalog API — with no second rebase later.

**My lane (deliberately not your lane):** provider-plane convergence. I'm porting PC2's `cenc-decrypt` Rust crate as the backend of the existing `decrypt-provider` capsule — the foundational decrypt primitive under the `decrypt → rights → key → drm` chain — behind the fail-closed provider boundary (apps get decrypted bytes, never keys). I'll also knock out the `bincode 1.3 → 2.x` security follow-up you flagged.

**I'm staying off** `library`/FileExplorer (done), `system`/Settings (yours this week), `marketplace` (done), and `provider/registry.rs` + `gateway_provider_proxy.rs` + `server_infra.rs` (your transfer rails — I'll consume, not edit).

Two asks: (1) confirm the lane split works for you, and (2) flag anything else you expect to touch this week beyond Settings so we don't collide. Heads-up that I'm mid-GitHub-suspension, so my commits are local until the account is back — then I'll open per-feature PRs against `0.4.0`.
