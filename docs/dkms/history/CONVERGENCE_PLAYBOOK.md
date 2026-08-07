# ElastOS Runtime Convergence Playbook (North Star)

> **Read this first.** This is the durable, first-principles alignment doc for the
> multi-month effort to bring the Elacity / PC2 product into the **ElastOS Runtime**
> as the Rust-powered engine. Any agent or contributor working on convergence should
> read this before touching code, and re-read it when a decision feels ambiguous.
>
> **Order of authority:** `PRINCIPLES.md` (the constitution) → this playbook (how we
> apply it to convergence) → per-task docs under `docs/dkms/history/` and
> `.cursor/tasks/`. If anything here ever contradicts `PRINCIPLES.md`, `PRINCIPLES.md`
> wins and this doc is wrong and must be fixed.
>
> **Companion docs:** `PRINCIPLES.md`, `docs/PC2_CONVERGENCE.md` (PC2→Runtime
> translation table), `V040_COORDINATION.md` (a current-week tactical plan, not retained).

---

## 1. The mission, stated plainly

Make the **operating system run through the Runtime** — as much of the product, as
natively, as the capability architecture allows. The Runtime is the small trusted
Rust core; everything else is a capsule or a provider behind it.

**Priority stack (what matters most, in order):**

1. **The capability substrate is correct.** Every feature is re-expressed as
   `capsule → capability → Carrier/provider plane → provider backend`. This is
   non-negotiable and comes before feature breadth.
2. **The Elacity dDRM system works inside the Runtime** — the ability to *package*
   capsules/content, *buy/trade* access, and *decrypt/render* protected content,
   entirely mediated by the must-do provider architecture. **This is the crown jewel**
   (see §6). It is more important than any individual app.
3. **File Explorer + Settings + the connected Home surfaces** run in the Runtime.
   (Anders owns these on the `0.4.0` line — we support, we do not duplicate.)
4. **AI and networking** become first-class capability planes. The architecture for
   these is *not yet settled* — treat as an open question (see §10), design later.

We are **not** chasing applications for their own sake. A working narrow slice of the
substrate beats a broad pile of ported apps.

---

## 2. First-principles non-negotiables (the decision rules)

Distilled from `PRINCIPLES.md`. When two implementations both "work," choose the one
that satisfies more of these:

1. **No ambient authority (#3).** Capsules start at zero authority and *request*
   capabilities. Authority is narrow, auditable, revocable. Missing authority **fails
   closed**.
2. **Everything through the Carrier/provider plane (#4).** Capsules speak capability
   calls, never raw sockets, host routes, IPFS/Kubo, chain RPC, or provider internals.
3. **Small trusted core (#5).** Trusted logic in the runtime; app logic in capsules;
   service logic in providers. Never let host/web plumbing become the product model.
4. **Fail closed, then explain (#11).** No silent downgrade, no half-implemented
   feature pretending to work. Errors say what is missing and what the correct path is.
5. **Docs, code, tests, ops agree (#12).** A boundary is only real when all four teach
   the same contract. Drift is a bug.
6. **One canonical path per operation (#10).** No soft alternate paths hiding competing
   behaviors.
7. **Trust travels with signed content (#15).** Trust anchors in DID/CID/hash/signature,
   not gateway location. **Decryption and license policy are mediated by an explicit
   provider, never reimplemented inside apps.** Encrypted content is normal, not special.
8. **UI is not authority (#16).** Opening a page ≠ holding a capability. APIs require the
   capability for that surface, not route shape or iframe placement.
9. **Humans and agents share one authority model (#7).** Every visible user action maps
   to the same capability-scoped operation an agent would use.

---

## 3. The capability model every feature must re-express against

```
Home      → launches capsules (mints launch capabilities only after it has Home authority)
Wallet    → owns user-facing authority
Inbox     → handles approvals
System    → policy / recovery / diagnostics / Settings / Storage
Runtime   → enforces capabilities
Carrier / provider plane → mediates ALL external effects
```

Apps **never** receive raw wallet, chain, node, IPFS, network, provider, or identity
authority. They request a capability; the runtime brokers it; the provider performs the
dangerous operation behind the boundary and returns **scoped output**.

---

## 4. The convergence laws (how we move PC2 → Runtime)

These are the rules of *this* effort. Break them and we create the exact technical debt
the project exists to avoid.

1. **One boundary at a time.** Never import a feature whose authority assumptions have
   not yet been re-expressed as capability requests. (This is why PC2's shared-identity
   chat code was held back from the Hey capsule.)
2. **Contract-first.** The capsule/provider contract ("the function signature of a
   capsule") is agreed *before* implementation. Where the Runtime already has a contract
   (e.g. `decrypt-provider`), the contract is the spec — implement the backend behind it.
3. **Characterization tests before rewrite.** Capture what PC2 *actually does* as
   language-agnostic golden fixtures (inputs → outputs), then make the Rust port pass
   them. This is the single most important de-risking move.
4. **Anti-Corruption Layer.** Translate PC2 models into the Runtime model at the seam.
   Never let PC2's "session-token-equals-everything" assumptions leak into the capability
   system.
5. **Carry the hardening forward.** PC2 v1.3 matured toward the capability world
   (capability-URL HMAC signing, live session revocation, **CEK never observable on the
   host**, decrypt inside a WASM sandbox). A port must **preserve or strengthen** these,
   never regress them.
6. **Translate, don't copy.** Reuse PC2 algorithms and WASM crates as **provider
   internals** behind the boundary. Do not copy PC2's iframe, broad-session, app-visible
   wallet, or direct-IPFS patterns (see `docs/PC2_CONVERGENCE.md` "What Not To Port").
7. **Don't duplicate in-flight work.** Anders owns File Explorer + Settings + marketplace
   + provider transfer rails on `0.4.0`. We **consume** those, we don't edit them. We
   pick complementary, non-colliding boundaries (the provider plane is ours).
8. **Reproducible, signed builds.** Pin the `wasm32-wasip1` target; sign manifest+binary
   (Ed25519); populate `sha256`/CID. Record the PC2 source commit for every port.

---

## 5. Named patterns we use (and anti-patterns we refuse)

**Use (field-tested migration patterns):**
- **Strangler Fig** (Fowler) — grow the new system around the old, route one capability
  at a time, never a flag-day cutover.
- **Branch by Abstraction** — both old and new impls satisfy the provider/capability
  interface; swap behind it.
- **Expand–Contract / Parallel Change** — add new path, migrate callers, remove old path
  (three steps, never one). Keeps `main` shippable.
- **Characterization / golden-file tests** (Feathers) — the spec both codebases honor.
- **Thin vertical slices / tracer bullets** — one capability end-to-end (UI → capability
  → provider → persistence → audit) before breadth.
- **Audit/observability from line one** — emit the receipt from the first capability call.

**Refuse:**
- Big-bang rewrite ("port it all, then switch").
- Porting ambient-authority assumptions into a capability system.
- Writing Rust that duplicates Anders' in-flight capsules.
- Pulling convenience features into the trusted core.
- Hooks in JSX, monolithic components, duplicated constants/types/utils (see the repo's
  code-quality rules for UI capsules).

---

## 6. The crown jewel: Elacity dDRM inside the Runtime

This is priority #2 and the architectural heart of the convergence. The goal: **package,
buy/trade, and decrypt protected content entirely through the provider plane**, with
keys never exposed to apps.

### 6.1 The must-do provider chain (fail-closed, in order)

```
drm/open  →  rights-provider   (is this principal entitled? produce an entitlement)
          →  key-provider      (release the CEK as a receipt, scoped + expiring)
          →  decrypt-provider  (decrypt/render INSIDE the boundary using the receipt)
          →  scoped output     (app/viewer gets rendered bytes — NEVER the CEK)
```

Each arrow is a capability call. The sequence is **already wired at the contract level**
in the Runtime: `capsules/decrypt-provider` defines `OpenSession`/`Render` over
`DecryptSessionRequestV1`, requires a `key-provider` `release_receipt_id`, and **blocks**
`raw_cek`, `raw_plaintext`, `filesystem`, `key_backend_sdk`, `kms_node_credentials`,
`chain_rpc`, `wallet_rpc`, `provider_credentials`. The gap is the **decrypt/render
backend** behind that contract.

### 6.2 The CEK-containment rule (the security invariant — never violate)

- The **CEK lives only inside the decrypt boundary**. It enters via a key-release receipt,
  is used to decrypt, is **zeroized** after use, and is **never** returned, logged, or
  surfaced — to apps, viewers, the host process broadly, or audit.
- The app receives **output scoped by `output_kind`** (e.g. `rendered`), never raw
  plaintext unless an explicitly allowed output kind says so.
- This mirrors PC2 v1.3's hardening (CEK never observable on host; decrypt in a WASM
  sandbox). The Runtime capsule **is** the sandbox boundary; a future hardening is to run
  the decrypt engine as a further nested WASM module (defense in depth).

### 6.3 Capsules/providers involved

| Concern | Capsule/provider | Status |
|---|---|---|
| Entitlement / license policy | `rights-provider` | scaffold (fail-closed) |
| Key release (CEK as scoped receipt) | `key-provider` | scaffold (fail-closed) |
| Decrypt + render | `decrypt-provider` | **contract done; backend is the work** |
| DRM orchestration | `drm-provider` | scaffold |
| Content availability | `availability-provider`, `content`/IPFS providers | present |
| Buy/trade (gated on chain) | `chain-provider`, `wallet-provider` | present; on-chain rights gating is a later boundary |

### 6.4 PC2 assets that map here (translate as provider internals)

- `pc2-node/crates/cenc-decrypt` — AES-128-CTR CENC/fMP4 segment decryptor, `wasm32-wasip1`,
  CEK kept in linear memory + zeroized. → the **decrypt-provider backend**.
- `ddrm-decrypt`, `ddrm-renderer` — full dDRM decrypt + render. → decrypt/render backend +
  viewer capsules.
- `.ddrm` content-hashed data capsule (content + viewer + Lit params). → "signed content +
  declared viewer" data capsule.

**Sequencing note:** the decrypt *mechanics* need only a (test) key release — they do
**not** require EVM/wallet. On-chain **rights acquisition** (buy/trade) is a later,
chain-gated boundary. Build and test decrypt against fixtures now; wire real rights/key
release as those providers come online.

---

## 7. Working agreements (this effort, today)

- **No push until reinstatement.** Git is distributed; we `git fetch` the public repo and
  base/rebase locally. Nothing is lost by working offline.
- **Branch topology:**
  - `sash/local-test-v030` — Mac VZ platform deliverable, **frozen** standalone review branch.
  - `sash/v040-integration` — tracks `origin/0.4.0`; rebase onto it as Anders pushes.
  - `feat/*` — one per convergence slice (e.g. `feat/decrypt-provider-cenc`).
  - `chore/*` — security follow-ups (e.g. `chore/bincode-2-migration`).
- **Stay current:** `git fetch origin && git rebase origin/0.4.0` on the integration branch,
  then rebase feature branches onto it.
- **End-of-week / on reinstatement:** per-feature PRs into `0.4.0`; Mac VZ as its own
  platform-decision PR.

---

## 8. The 10/10 execution standard

**Definition of done for any convergence slice:**
- [ ] Contract identified or written (the capsule/provider interface it implements).
- [ ] Characterization/golden fixtures captured *before* the rewrite; they pass.
- [ ] Fail-closed paths preserved and tested (missing/invalid authority → explicit error).
- [ ] Security invariant proven by test (e.g. CEK never returned/logged).
- [ ] `blocked_authority` / capability scopes honored; no ambient authority added.
- [ ] PC2 source commit recorded (verification rule, §9).
- [ ] Branch rebased onto latest `origin/0.4.0`; one-page sync note for Anders updated.
- [ ] Docs/code/tests agree; no drift introduced.

**Daily operating rhythm:**
- Work proceeds in **days**. Each day is a thin, demoable increment.
- At the **end of each day**: report what was achieved, what (if anything) the user must
  test or confirm, and present the **10/10 prompt for the next day** with a request to
  type `continue`.
- The Runtime UI is kept runnable locally (`elastos gateway`) so progress is explorable.

---

## 9. Verification rule (before importing any PC2 pattern)

Name, in writing:
- the Runtime principal/session/capability it depends on,
- the provider/connector capsule that owns the dangerous authority,
- the app-visible API that stays protocol-agnostic,
- the fail-closed test that proves apps cannot bypass the provider plane,
- the PC2 source commit / pinned artifact used as reference.

If you cannot name all five, the boundary is not ready to cross.

---

## 10. Open questions (design later, do not improvise)

- **AI plane.** How AI/agent compute is expressed as a capability plane (`compute:ai-*`),
  with the same authority/audit/resource boundaries as humans (`PRINCIPLES.md` #7). Mirror
  the Hey "identity projection + app-scoped storage" pattern. **Not yet settled.**
- **Networking plane.** How off-box peer/transport (Carrier generation, iroh/Hickory) and
  content availability evolve. The Carrier-generation upgrade is decoupled debt that plays
  to carrier-bridge expertise. **Not yet settled.**

When these are designed, they get their own contract docs and follow §4 and §8.

---

*This is a living document. Update it as contracts are confirmed, boundaries are crossed,
and the AI/networking architecture is decided. Keep it honest — it is only useful if it
describes how we actually work.*
