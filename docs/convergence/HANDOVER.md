# ElastOS Runtime — Convergence Handover (read me first)

**Purpose.** This is the single entry point for a new engineer/agent picking up the
ElastOS Runtime ⇄ PC2 convergence work in a fresh context window. Read this top to
bottom once; it tells you exactly what we're doing, why, what's done, what to read,
and how to continue at the same quality bar — with no loss of insight.

**Last updated:** 2026-06-08 (end of Day 24).
**Active branch:** `feat/decrypt-provider-cenc` (tip `8cb43b814`).
**Repo:** `/Users/sash/code/elastos-runtime` (this repo).
**PC2 reference repo (stable source of truth):** `/Users/sash/Documents/Cursor/pc2.net/pc2-node`.

---

## 0. The 30-second picture

We are re-platforming the Elacity web product (**PC2 / pc2.net**) onto a
**capability-secure Rust runtime** (this repo, ElastOS). The crown jewel is
**dDRM** — decentralized DRM — a fail-closed provider chain that lets an app *see*
protected content while never letting it *hold* the keys.

Over Days 1–17 we brought the **entire dDRM provider chain to a proven,
fail-closed, wasm-built, contract-tested bar**, pinned **both** security invariants
(encrypt + decrypt), proved the **post-quantum crypto compiles in wasm**, and made
the work **rebase-safe** against Anders' in-flight 0.4.0. Days 18–24 then advanced
every *unblocked* edge of the rail right up to the transport decision: closed the
encrypt in-boundary-keygen gap, de-risked the PQ-hybrid envelope, **proved the full
PQ dDRM data path end-to-end pre-rail**, locked the engines with **portable golden
vectors**, and made the "byte-compatible with PC2" claim **executable** via a
standing cross-impl conformance gate (`ddrm-verify.sh`). Everything is isolated on
local branches because **GitHub push access is suspended** (see §6).

The chain is **blocked on exactly one architectural decision from Anders** (the CEK
transport rail — `DDRM_DECRYPT_RAIL.md`). Everything else that depended on it has
been pinned, de-risked, or proven pre-rail: the only remaining work behind the
blocker is the **transport shim** that wires the (already-proven) unwrap→cenc
composition to whatever rail Anders confirms. Everything that *can* be done ahead of
that answer, *is* done.

---

## 1. Mission & priority stack

From `CONVERGENCE_PLAYBOOK.md` (the north star — read it second):

1. **dDRM is the crown jewel.** Protected-content economy is the product's reason to
   exist. Everything else serves it.
2. **Capability security is non-negotiable.** Small trusted Rust core; everything
   else is an isolated capsule/provider with zero ambient authority; fail-closed.
3. **Contract-first convergence.** PC2 is the stable behavioural reference; we
   translate its *patterns* into the capability model, we do not copy its trust
   assumptions. Pin contracts with characterization tests before wiring engines.
4. **One boundary at a time. Isolated, reversible, reviewable.**

---

## 2. The mental model (how the system is shaped)

**ElastOS is a capability OS.** Isolation tiers, highest authority first:

| Tier | Tech | Isolation | Examples |
|---|---|---|---|
| Trusted core | Rust, native host | the runtime process | the Runtime itself |
| **Providers** | Rust, `type: microvm` | full VM (crosvm/Linux, Apple VZ/macOS) | decrypt, key, rights, drm, encrypt, wallet, ai… |
| Shells / system logic | Rust → `wasm32-wasip1`, `type: wasm` | wasmtime sandbox | Home, System |
| App / content / UI | Web (HTML/JS), `type: data` | runtime-mediated browser principal | Library, Marketplace |

**The dDRM chain** (the spine of the crown jewel):

```
app/viewer --drm/open--> drm-provider --> rights-provider --> key-provider --> decrypt-provider --scoped output--> player
                                          (RightsDecisionReceipt) (ReleaseReceipt + sealed CEK)        (NO CEK ever)
```

- Authority is passed between stages as **signed receipts**, never as keys.
- The **CEK** (content key) is the only true secret. It travels **sealed**, is
  unwrapped/used/zeroized **inside one boundary** (decrypt-provider), and **never**
  reaches the player.
- Two **viewer/player** kinds consume scoped output: **media** (video/audio → fMP4
  segments) and **non-media** (pdf/epub/cbz/images → rendered/plaintext). Both get
  an opaque handle, never the CEK. (Ross built these in PC2.)

**Irzhy's two security invariants (binding):**
- **#1 (encrypt):** CEK+KID generated **inside** a wasm boundary; only ciphertext +
  non-secret relatives output.
- **#2 (decrypt):** CEK **never** passed as plaintext to other components; recovery
  + decryption colocated in one boundary + zeroize at end.

---

## 3. What's built (current truth)

The four-stage chain **plus** the encrypt producer, all fail-closed and wasm-built:

| Provider | Role | Host tests | wasm | Notes |
|---|---|---|---|---|
| `capsules/encrypt-provider` | seal/produce (invariant #1) | 13 | builds | self-contained; **in-boundary CEK+KID keygen closed** (Day 19) |
| `capsules/drm-provider` | orchestrator `drm/open` + chain-seam | 12 | builds | declares canonical open sequence |
| `capsules/rights-provider` | rights decision | 9 | builds | wire-rejects hidden authority |
| `capsules/key-provider` | key release (rights-bound) | 9 | builds | verifies upstream RightsDecisionReceipt |
| `capsules/decrypt-provider` | decrypt/render (invariant #2) | 25 | builds | cenc engine + envelope spec + consumer contract |

**68 host tests green; 0 ignored** (Day 19 closed the encrypt keygen gap: 6+1-ignored → 13).

The `decrypt-provider` also carries **feature-gated tested islands** (Parallel
Change — off by default, so the base surface above is unchanged). Cumulative test
counts per feature:

| `cargo test --features …` | count | what it adds |
|---|---|---|
| *(default)* | 25 | the shipped decrypt contract |
| `rail-prep` | 27 | classical `ecdh_unwrap → cenc` composition (Day 18) |
| `pq-envelope` | 29 | PQ-hybrid CEK-seal envelope island (Day 20) |
| `pq-rail-prep` | 31 | full PQ data path `hybrid_unwrap → cenc` (Day 21) |
| `vectors` | 36 | replay portable golden vectors v3+v2 (Days 22, 24) |
| `gen-vectors` | — | regenerate the committed vectors (writes `tests/vectors/`) |

Proven properties (all test-backed — see `DDRM_SECURITY_MODEL.md` §9):
- Zero ambient authority surfaced; every provider advertises + wire-rejects raw
  authority (`deny_unknown_fields`).
- Fail-closed by default (`not_configured` until a real backend exists).
- **CEK containment + zeroization** at both ends.
- **Invariant #1 closed:** `encrypt-provider` mints CEK+KID with a CSPRNG **inside**
  the boundary and the seal engine emits no key material (Day 19).
- **Authorization binding** (rights receipt → key release).
- **Contracts compose** (cross-provider seam tests).
- **Upstream rail contract** captured as an executable spec
  (`decrypt-provider/src/envelope.rs`: P-256 ECDH unwrap → AES-256-CBC, vendored
  from PC2 `ddrm-decrypt`).
- **Downstream consumer contract** pinned for both players (metadata-only output).
- **PQ-hybrid compiles in wasm** (`ml-kem 0.2.3`, `ml-dsa 0.0.4` → `wasm32-wasip1`,
  Rust 1.89). GO with a pin-exact caveat (`ml-dsa` is 0.0.x).
- **Full PQ dDRM data path proven pre-rail** (Day 21): `pq_envelope.rs`
  `decrypt_pq_sealed_segment` chains `x25519+ml-kem-768` hybrid unwrap → cenc
  decrypt, CEK in `Zeroizing` throughout, never on the boundary.
- **Engines pinned by portable golden vectors** (Days 22, 24): substrate-independent
  fixtures in `decrypt-provider/tests/vectors/` (classical v3 + v2, and PQ-hybrid)
  replayed with no in-test sealing and no RNG.
- **Cross-impl conformance is executable** (Days 23–24): `scripts/pc2-conformance.sh`
  decrypts our committed vectors with PC2 `ddrm-decrypt`'s **real code** and asserts
  byte-for-byte parity (CEK + plaintext) plus fail-closed parity on tamper, for both
  envelope versions. Skips clean when PC2 is absent.

---

## 4. Document map — what to read, in order

All in `docs/convergence/`. Read 1→3 to onboard; the rest are reference.

1. **`HANDOVER.md`** (this file) — start here.
2. **`CONVERGENCE_PLAYBOOK.md`** — north star: mission, priority stack, decision
   rules, capability model, convergence laws, migration patterns, the 10/10 bar.
3. **`DDRM_STATUS.md`** — current truth: parity table, proven properties, commit
   inventory, the open rail decision, base-reconciliation status. **Refresh this as
   you work.**
4. **`DDRM_SECURITY_MODEL.md`** — the trust model: actor/boundary map, encrypt +
   decrypt mermaid flows, threat model, PQ crypto profile, invariant→test table.
5. **`DDRM_DECRYPT_RAIL.md`** — the one open architecture decision (how the CEK
   reaches decrypt) with options, recommendation, and the sharpened questions for
   Anders. **This is the blocker.**
6. **`DDRM_ENCRYPT_INVARIANT.md`** — encrypt side (invariant #1): the PC2
   host-keygen gap, the target contract, the scoped landing.
7. **`PC2_PLAYER_ALIGNMENT.md`** — media vs non-media players mapped to tiers;
   Irzhy's invariants validated; the ECDH envelope as the concrete rail evidence.
8. **`PRODUCT_VISION.md`** — the PRD: what ElastOS is, personas, pillars, roadmap.
9. **`PUSH_PLAN.md`** — how to land the local branches as PRs when GitHub returns,
   **including the rebase recipe** for the force-pushed 0.4.0.
10. **`V040_COORDINATION.md`** — tactical week plan + division of labour with Anders.
11. **`MAC.md` / `RUN_HOME_LOCALLY.md`** (in `docs/`) — run the UI locally on macOS
    (`elastos gateway` not `serve`; use `localhost:8090` for WebAuthn).

**Full prior conversation transcripts** (every decision, verbatim) — search by
keyword (filename, error, "Day N") if you need the why behind a decision:
- Days 1–17: `…/agent-transcripts/6f8c08cd-415d-4f58-b41d-74e2724fb796/6f8c08cd-415d-4f58-b41d-74e2724fb796.jsonl`
- Days 18–24: `…/agent-transcripts/43110c1d-e79d-43d4-818b-4a2f0fb3233b/43110c1d-e79d-43d4-818b-4a2f0fb3233b.jsonl`

(both under `/Users/sash/.cursor/projects/Users-sash-code-elastos-runtime/`)

---

## 5. Key people & their concerns

- **Anders** — runtime lead. Owns the 7 binding Mac-VZ decisions and the 0.4.0
  mainline. **Actively redoing 0.4.0 today** (only ~20% on GitHub; force-pushed
  once already; more redones coming). Owns the **rail decision** (DDRM_DECRYPT_RAIL
  §"Questions for Anders"). Do **not** rely on the GitHub 0.4.0 being final.
- **Irzhy** — security. Author of the two invariants (§2). Proposed "two boxes +
  secured channel (ECDH + DSA)" for the key→decrypt hop — we adopted it, upgraded to
  PQ-hybrid. He had requested a clearer picture → that's `DDRM_SECURITY_MODEL.md`.
- **Ross** — built PC2's media + non-media players (the consumers of our decrypt
  output). Their contract is pinned in `PC2_PLAYER_ALIGNMENT.md`.

---

## 6. Critical constraints (do not forget these)

- **GitHub push is SUSPENDED** (user's account). All work lives on **local
  branches**; nothing is pushed. We can `git fetch` (read) but not push. Plan to
  push = `PUSH_PLAN.md`.
- **0.4.0 is in flux.** Anders force-pushed it and more redones are coming. **Do not
  rebase onto it yet.** When it settles: run `scripts/ddrm-verify.sh` (drift +
  cross-impl conformance, must PASS), then follow the rebase recipe in
  `PUSH_PLAN.md`. A safety backup of our tip is `backup/decrypt-provider-cenc-preD17`.
- **Contract converged — zero type drift.** `elastos-common/protected_content.rs`
  is byte-identical between our branch and the redone 0.4.0. Our providers were
  built against the exact types Anders independently landed. Keep it that way; the
  drift guard enforces it.
- **MSRV is pinned 1.89** (`rust-toolchain.toml`). The Carrier `iroh`/Hickory CVE
  closure needs 1.91 → it's a deferred operator decision (`CARRIER_IROH_UPGRADE.md`).
- **PC2 uses classical crypto (P-256 ECDH/ECDSA); the Runtime mandates PQ-hybrid**
  (`x25519+ml-kem-768`, `ml-dsa-65`). Keep PC2's envelope *structure*, upgrade the
  *crypto*.
- **`encrypt-provider` is intentionally self-contained** (no `elastos-common` dep)
  to survive 0.4.0 churn — reconcile once it stabilises (drift-check prints the list).

---

## 7. Branch topology (local, unpushed)

dDRM + convergence work (all based on `origin/0.4.0`):

| Branch | What | Push order (PUSH_PLAN) |
|---|---|---|
| `feat/decrypt-provider-cenc` | **the main dDRM branch** (Days 1–17): 5 providers, engine, envelope spec, consumer contract, security model, drift guard, all docs | #5 (the big one) |
| `fix/crosvm-darwin-build` | gate Linux-only TAP networking so 0.4.0 builds on macOS | #1 |
| `fix/home-summary-resilience` | corrupt `browser-state.json` resets instead of failing login | #2 (stacked on #1) |
| `chore/bincode-2x` | bincode 1.3→2.x with wire-format compat tests | #3 |
| `chore/carrier-iroh-upgrade` | iroh/Hickory ADR + audit.toml rationale (docs only) | #4 |
| `backup/decrypt-provider-cenc-preD17` | safety snapshot before the Day-17 base analysis | — (do not push) |

Older/unrelated: `sash/local-test*` (Mac VZ core work, intentionally separate),
`chore/runtime-cve-hygiene*`, `sash/v040-integration`.

---

## 8. Open items / what's NOT done (and why)

1. **The decrypt rail** (BLOCKER, needs Anders). How the sealed CEK reaches
   decrypt-provider. We chose Hybrid (decrypt *receives* sealed material; upstream
   is a provider chain) + Irzhy's secured ECDH+DSA channel, PQ-hybrid. **3 sharpened
   questions for Anders** in `DDRM_DECRYPT_RAIL.md` / `DDRM_STATUS.md`. The full
   unwrap→cenc composition is proven for both the classical (`rail-prep`) and PQ
   (`pq-rail-prep`) profiles — so once answered, the remaining work is the
   **transport shim**, not the crypto or the engines.
2. ~~Encrypt in-boundary keygen engine (invariant #1 gap).~~ **CLOSED (Day 19).**
   `encrypt-provider` now mints CEK+KID with a CSPRNG inside the boundary and the
   seal engine emits no key material; `cek_and_kid_generated_inside_boundary` and
   `seal_engine_emits_no_key_material` pass. See `DDRM_ENCRYPT_INVARIANT.md`.
3. **PQ migration of the envelope** (de-risked, not yet shipped). `envelope.rs` is
   the classical PC2 spec; `pq_envelope.rs` proves the PQ-hybrid profile end to end
   behind `pq-envelope`/`pq-rail-prep`. Wiring it into dispatch lands with the rail.
4. **Carrier iroh/Hickory upgrade** — deferred (MSRV 1.91), operator decision.
5. **Rebase onto stabilised 0.4.0** — deferred until Anders stops force-pushing.
   Pre-rebase gate is now `scripts/ddrm-verify.sh` (drift + cross-impl conformance).

---

## 9. How to verify (commands)

```bash
# THE standing pre-rebase/PR gate: contract drift + PC2 cross-impl conformance.
# Conformance skips clean if the PC2 repo is absent; set PC2_REPO to point it.
scripts/ddrm-verify.sh                          # expect: ALL GATES PASS

# (the gate's two parts, runnable on their own)
scripts/ddrm-drift-check.sh                     # expect PASS
scripts/pc2-conformance.sh                      # expect PASS (or SKIP without PC2)

# per-provider host tests (fast, authoritative): 13+12+9+9+25 = 68 green, 0 ignored
for p in encrypt drm rights key decrypt; do (cd capsules/$p-provider && cargo test); done

# decrypt-provider feature ladder (tested islands; counts in §3)
( cd capsules/decrypt-provider && \
  for f in rail-prep pq-envelope pq-rail-prep vectors; do cargo test --features $f; done )

# regenerate the committed golden vectors (only when intentionally changing them)
( cd capsules/decrypt-provider && cargo test --features gen-vectors emit_ )

# whole chain under the WASI sandbox (needs: rustup target add wasm32-wasip1; brew install wasmtime)
scripts/ddrm-chain-smoke.sh                     # 4 chain providers PASS

# wasm build of a provider
( cd capsules/decrypt-provider && rustup run 1.89.0 cargo build --target wasm32-wasip1 --release )
```

---

## 10. The working method (how we operate — keep this bar)

**Convergence laws** (from the playbook):
- One boundary at a time; isolated commits; reversible.
- Contract-first: pin the interface with **characterization tests** before wiring an
  engine.
- Anti-Corruption Layer: translate PC2 patterns, don't import its trust model.
- CEK containment is sacred: it lives sealed, is used in one boundary, zeroized.
- Fail-closed: every unconfigured path returns `not_configured`, never opens.

**Validate against the source of truth.** When mapping a PC2 behaviour, read the PC2
repo (`/Users/sash/Documents/Cursor/pc2.net/pc2-node`), not memory. Key PC2 crates:
`crates/cenc-encrypt`, `crates/ddrm-decrypt`, `wasm-apps/ddrm-renderer`,
`src/services/media/dashPackager.ts`.

**Commit discipline.** Small, scoped, descriptive commits on the right isolated
branch. Never push (suspended). Never commit `build/` or `scripts/dev/`.

---

## 11. The "10/10 daily prompt" methodology (important)

The user runs this as a **day-by-day loop**. At the end of each day you:
1. Report what was done (crisp, evidence-backed: tests green, commit SHA, branch
   ahead-count).
2. **Present the next day's "10/10 prompt"** and ask the user to continue.

A **10/10 prompt** is engineered with this anatomy:
- **Role** — frame the agent as a senior specialist (e.g. "Convergence lead").
- **Objective** — the single highest-leverage, *unblocked* outcome for the day,
  justified by the priority stack and current blockers.
- **Tasks** — 2–4 concrete, ordered steps; validate against PC2 + the runtime
  principles; pin with characterization tests; keep isolated.
- **Definition of done** — measurable: tests green / proof recorded / one isolated
  commit / docs updated.
- Implicitly: best-practice framing (industry standards, named patterns —
  Strangler Fig, ACL, Branch-by-Abstraction, characterization tests), and always
  rebase-safe + fail-closed + contract-first.

The loop's discipline: **never do blocked work**; always advance the most valuable
thing that is *currently* unblocked; leave the chain provably green; document so the
next context can continue cold.

---

## 12. Day log (1–17, one line each)

- **D1** vendor PC2 cenc engine into decrypt-provider; fix typed `release_receipt`.
- **D2** gate Linux-only crosvm networking → 0.4.0 builds on macOS (`fix/crosvm-darwin-build`).
- **(bugfix)** passkey 500 = corrupt `browser-state.json`; resilient reset (`fix/home-summary-resilience`).
- **D3** decrypt-step core seam (Branch-by-Abstraction) + rail decision recorded.
- **D4–5** decrypt-provider wasm/WASI proofs + isolation-tier rationale.
- **D6** key-provider binds upstream rights receipt; wasm/WASI bar.
- **D7** rights-provider WASI smoke (chain parity).
- **D8** drm-provider WASI smoke + cross-provider contract-seam tests.
- **D9** unified `ddrm-chain-smoke.sh` + review-ready `DDRM_STATUS.md`; architecture visuals.
- **D10** bincode 1.3→2.x with wire-format golden tests (`chore/bincode-2x`).
- **D11** Carrier iroh/Hickory upgrade ADR — blocked on MSRV (`chore/carrier-iroh-upgrade`).
- **D12** vendor ECDH envelope spec + PC2 player alignment; `PUSH_PLAN.md`.
- **D13** pin decrypt→player consumer contract (both players).
- **D14** `DDRM_SECURITY_MODEL.md` (flows, threat model, invariant→test) + inter-stage CEK transport decision (Irzhy).
- **D15** refresh status; prove PQ-hybrid compiles in wasm (ml-kem/ml-dsa).
- **D16** `encrypt-provider` skeleton; pin invariant #1; capture in-boundary-keygen gap.
- **D17** 0.4.0 force-push reconciled (zero type drift); `ddrm-drift-check.sh`; deferred rebase.
- **D17.5** `HANDOVER.md` single-entry onboarding (`14cb2306d`).
- **D18** prep rail-landing: classical `ecdh_unwrap → cenc` composition behind `rail-prep` (`27cce2d5e`).
- **D19** close invariant #1: in-boundary CEK+KID keygen + seal engine; 68 green/0 ignored (`ec6fd6dcf`).
- **D20** de-risk PQ-hybrid CEK-seal envelope island behind `pq-envelope` (`38fa91a48`).
- **D21** prove full PQ dDRM data path end-to-end pre-rail behind `pq-rail-prep` (`ee5b084f9`).
- **D22** pin both engines with portable golden vectors (`vectors`/`gen-vectors`) (`7df180297`).
- **D23** make PC2 cross-impl conformance executable (`scripts/pc2-conformance.sh`) (`8bf242a20`).
- **D24** promote conformance to a standing gate (`ddrm-verify.sh`) + v2 vector + tamper parity (`8cb43b814`).

---

## 13. Next

The Days 18–24 options (rail-prep, encrypt keygen, PQ envelope, PQ data path,
golden vectors, executable conformance) are **done**. The rail itself remains
blocked on Anders. The next-day prompt is normally provided by the prior agent; if
you do not have it, the highest-value **unblocked** options, in order, are:

1. **Widen the conformance/golden surface toward the live rail's wire shape** — e.g.
   PQ-vector cross-checks once a reference exists, multi-sample/subsample cenc
   vectors, or an `init`-segment (`tenc`) vector — so more of the contract is pinned
   by executable parity before wiring.
2. **Author the rail transport shim behind a flag, fully tested, un-wired** — the
   thin adapter that hands a sealed envelope to `decrypt_pq_sealed_segment`
   (`pq-rail-prep`) for each rail option in `DDRM_DECRYPT_RAIL.md`, so the day Anders
   answers it is a flag flip, not a design.
3. **Reconcile-prep for 0.4.0** — keep `ddrm-verify.sh` green; tighten the drift
   guard / `PUSH_PLAN.md` rebase recipe so the eventual rebase is button-press.
4. **Encrypt→decrypt round-trip golden** — a vector produced by `encrypt-provider`'s
   in-boundary seal and consumed by the decrypt engines, pinning both invariants on
   one artifact.

Whatever you pick: keep it isolated on `feat/decrypt-provider-cenc`, pin it with
characterization tests, keep the gate green (`scripts/ddrm-verify.sh` + the 68
provider tests), update `DDRM_STATUS.md`, and end the day by presenting the next
10/10 prompt.
