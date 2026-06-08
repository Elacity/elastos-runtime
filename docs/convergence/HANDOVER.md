# ElastOS Runtime — Convergence Handover (read me first)

**Purpose.** This is the single entry point for a new engineer/agent picking up the
ElastOS Runtime ⇄ PC2 convergence work in a fresh context window. Read this top to
bottom once; it tells you exactly what we're doing, why, what's done, what to read,
and how to continue at the same quality bar — with no loss of insight.

**Last updated:** 2026-06-09 (end of Day 40).
**Active branch:** `feat/decrypt-provider-cenc` (tip `b3b5f0a9d` + Day-40 integrity audit, ~44 commits ahead of `origin/0.4.0`).
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
standing cross-impl conformance gate (`ddrm-verify.sh`). Days 25–28 closed the gap to
the rail itself: an **encrypt→decrypt round-trip golden** (both invariants on one
artifact), the **rail transport shim behind a flag** (`rail-shim`) so the rail is now
a *flag-flip not a design*, and the shim's **carrier wire shape pinned as a portable
golden** that is also driven through **PC2's real session API** (`unwrap_envelope` →
`media::decrypt_segment`). Days 29–34 then made the **crypto core feature-complete,
PQ-proven, and adversarially hardened**: widened the cenc goldens to **real playback
shapes** (multi-sample / subsample / non-default-IV via init-`tenc`, all PC2-conformant);
**replaced the PQ signature stub with the real FIPS 204 `ml-dsa-65` primitive** (RustCrypto
`ml-dsa`, verify-only, `wasm32-wasip1`-clean) and proved it **through the exact
`decrypt_from_carrier` rail entrypoint** on a committed real-signed carrier golden; and
added an **adversarial negative-space + containment sweep** (`harden`) proving the
untrusted-input decoders fail closed and never panic. The verification gate
(`ddrm-verify.sh`) is now **authoritative over the whole ladder** (asserted test counts +
wasm builds). Everything is isolated on local branches because **GitHub push access is
suspended** (see §6).

The chain is **blocked on exactly one architectural decision from Anders** (the CEK
transport rail — `DDRM_DECRYPT_RAIL.md`). Everything that depended on it has been
pinned, de-risked, or proven pre-rail — *including the transport shim itself and the real
PQ signature primitive*, both built and fully tested. The remaining PQ items are now pure
**policy** (Anders' Q2: straight `ml-dsa-65` vs hybrid during PC2's migration), not build
gaps. The only work left behind the blocker is the **one-line `OpenSession` wire-up**
(`rail_shim::decrypt_from_carrier(...)`, passing a `MlDsa65Verifier`) once Anders confirms
who mints the carrier. Everything that *can* be done ahead of that answer, *is* done.

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
| `capsules/encrypt-provider` | seal/produce (invariant #1) | 13 | builds | **in-boundary CEK+KID keygen closed** (Day 19); output reconciled to shared `SealedObjectV1` (Day 39) |
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
| `vectors` | 42 | replay portable goldens: v3+v2, encrypt↔decrypt round-trips (single + **multi-sample + subsample**), multi-sample/subsample/init-IV cenc (Days 22, 24, 26, 31, 37) |
| `rail-shim` | 45 | carrier→engine adapter (`decrypt_from_carrier`) + carrier goldens, both profiles (Days 27–30) |
| `pq-mldsa` | 34 | real FIPS 204 ML-DSA-65 verifier in the `CekSealVerifier` slot + KAT (Day 32) |
| `rail-shim-mldsa` | 54 | the real ML-DSA-65 verified through `decrypt_from_carrier` on a committed carrier golden (Day 33) |
| `harden` | 65 | adversarial negative-space + containment sweep over the wire-decoders (Day 34) |
| `gen-vectors` | — | regenerate the committed vectors (writes `tests/vectors/`) |

The standing gate `scripts/ddrm-verify.sh` now asserts **all** of these counts +
the wasm builds (gate 3, `ddrm-ladder-check.sh`), so a dropped/feature-gated-out
test fails the gate rather than passing silently.

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
- **PQ-hybrid is real, not stubbed, and wasm-clean** (Day 32): the signature is the
  **real FIPS 204 ML-DSA-65** (`ml-dsa 0.1`, RustCrypto — same family as `ml-kem 0.2.3`),
  verify-only + rng-free so it builds to `wasm32-wasip1`. `pq_envelope::mldsa::MlDsa65Verifier`
  fills the `CekSealVerifier` slot; pinned by a committed deterministic KAT
  (`mldsa65_kat.json`) + fail-closed tests. The PQ signature is no longer a build gap —
  only Anders' Q2 transition *policy* remains.
- **Full PQ dDRM data path proven pre-rail** (Day 21): `pq_envelope.rs`
  `decrypt_pq_sealed_segment` chains `x25519+ml-kem-768` hybrid unwrap → cenc
  decrypt, CEK in `Zeroizing` throughout, never on the boundary.
- **Engines pinned by portable golden vectors** (Days 22, 24): substrate-independent
  fixtures in `decrypt-provider/tests/vectors/` (classical v3 + v2, and PQ-hybrid)
  replayed with no in-test sealing and no RNG.
- **Cross-impl conformance is executable** (Days 23–24, 28, 31, 38): `scripts/pc2-conformance.sh`
  decrypts our committed vectors with PC2 `ddrm-decrypt`'s **real code** and asserts
  byte-for-byte parity (CEK + plaintext) plus fail-closed parity on tamper, for both
  envelope versions — at **two layers**: the crypto primitives (`envelope`+`cenc`)
  and PC2's **public session API** (`session::unwrap_envelope` → `media::decrypt_segment`,
  the carrier path), over single/multi-sample/subsample/init-IV shapes. **And the
  producer half** (Day 38): the segments `encrypt-provider`'s real engine emitted
  (multi-sample + subsample) are decrypted by PC2's `mp4box`+`cenc` to the producer's
  exact bytes (+ wrong-CEK key-bound check) — proving PC2 consumes *our producer's
  output*, not only our consumer. Skips clean when PC2 is absent.
- **Both invariants pinned on one artifact, over real playback shapes** (Days 26, 37):
  `encrypt-provider`'s real in-boundary engine emits round-trip goldens —
  `roundtrip_encrypt_to_decrypt.json` (single sample) plus
  `roundtrip_multisample_encrypt_to_decrypt.json` (4 samples, per-sample IVs) and
  `roundtrip_subsample_encrypt_to_decrypt.json` (16-byte clear leader + encrypted
  body) — which `decrypt-provider` replays back to the producer's exact plaintext
  (`vectors`), CEK contained. Producer mux mirrors PC2 `cenc-encrypt::mp4box`
  (`build_senc` / `build_senc_with_subsamples`); the gate exercises all three by name.
- **The rail is a flag-flip, not a design** (Days 27–28): `decrypt-provider/src/rail_shim.rs`
  (`rail-shim`, default OFF, **not** wired into dispatch) is the carrier→engine adapter
  for rail Option A — `decrypt_from_carrier(session, carrier, verifier)` routes a sealed
  CEK + segment to the proven classical/PQ engines. Its carrier wire shape is pinned by
  a portable golden (`rail_carrier_classical.json`) and validated against PC2's session
  model. Q1 (who seals) doesn't touch it; Q2 (signature) plugs in via `CekSealVerifier`.
  The day Anders answers, `OpenSession` adds **one line**. (`DDRM_DECRYPT_RAIL.md` §"Rail
  transport shim".)
- **Media goldens widened to real playback shapes** (Day 31): multi-sample, subsample
  (clear+encrypted ranges), and a 16-byte-IV-via-init-`tenc` vector — each replayed
  through our engine **and** PC2's real `cenc`/`media::decrypt_segment` (byte parity +
  tamper fail-closed). `ClassicalVector` gained optional `init_segment_b64`/`iv_size`.
- **Real ML-DSA-65 verified through the rail entrypoint** (Day 33): a committed carrier
  golden (`rail_carrier_pq_mldsa.json`) whose seal signature is a genuine ML-DSA-65 sig,
  replayed through `decrypt_from_carrier` verified by the production `MlDsa65Verifier`
  (`rail-shim-mldsa`) — plaintext recovered + fail-closed on tampered sig / wrong key /
  tampered body. *The real PQ signature, through the real rail entrypoint, on a portable artifact.*
- **Fail-closed + panic-free under adversarial input** (Day 34, `harden`): truncation,
  single-byte-flip, and oversized-length-prefix sweeps over `envelope::parse`,
  `PqSealedEnvelope::from_bytes`, and the `decrypt_from_carrier` dispatch — every malformed
  shape fails closed, never panics, never recovers a CEK; error/metadata surfaces leak no
  plaintext/CEK; profile/secret mismatch fails closed both directions.

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
- Days 18–34: `…/agent-transcripts/43110c1d-e79d-43d4-818b-4a2f0fb3233b/43110c1d-e79d-43d4-818b-4a2f0fb3233b.jsonl`

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
- **`encrypt-provider` is reconciled to `elastos-common`** (Day 39): its sealed
  **output** is the shared `SealedObjectV1`/`KeyEnvelopeV1`; only the **input**
  `SealRequest` stays local (no shared seal-request type yet). The Day-16
  self-containment is retired (the contract is stable + drift-pinned).

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
   (`pq-rail-prep`) profiles, **and the transport shim itself is now built + fully
   tested** behind `rail-shim` (Days 27–28: `decrypt_from_carrier`, carrier golden,
   PC2 session conformance). So the only work left behind the blocker is the
   **one-line `OpenSession` wire-up** (`rail_shim::decrypt_from_carrier(...)`) — Q1
   (dKMS-direct vs re-seal) doesn't touch the adapter, Q2 (signature) plugs in via
   the `CekSealVerifier`, profile is a per-deployment `SealProfile` pick.
2. ~~Encrypt in-boundary keygen engine (invariant #1 gap).~~ **CLOSED (Day 19).**
   `encrypt-provider` now mints CEK+KID with a CSPRNG inside the boundary and the
   seal engine emits no key material; `cek_and_kid_generated_inside_boundary` and
   `seal_engine_emits_no_key_material` pass. See `DDRM_ENCRYPT_INVARIANT.md`.
3. **PQ migration of the envelope** (de-risked + real primitive, not yet wired into
   default dispatch). `envelope.rs` is the classical PC2 spec; `pq_envelope.rs` proves
   the PQ-hybrid profile end to end behind `pq-envelope`/`pq-rail-prep`, and the signature
   is now the **real FIPS 204 ML-DSA-65** (`pq-mldsa`/`rail-shim-mldsa`, Days 32–33), not a
   stub. Wiring it into default dispatch lands with the rail; the remaining choice is
   Anders' Q2 *policy* (straight ML-DSA vs hybrid during PC2's migration), not a build gap.
4. **Carrier iroh/Hickory upgrade** — deferred (MSRV 1.91), operator decision.
5. **Rebase onto stabilised 0.4.0** — deferred until Anders stops force-pushing.
   Pre-rebase gate is now `scripts/ddrm-verify.sh` (drift + cross-impl conformance +
   ladder counts/wasm; `DDRM_VERIFY_FAST=1` skips the heavy ladder gate).

---

## 9. How to verify (commands)

```bash
# THE standing pre-rebase/PR gate: drift + PC2 conformance + ladder/wasm + WASI smoke.
# Gate 3 (ladder) asserts every test count + the wasm builds, so a dropped or
# feature-gated-out test FAILS the gate. Gate 2 (conformance) skips clean without PC2;
# gate 4 (WASI smoke) skips clean without wasmtime.
scripts/ddrm-verify.sh                          # expect: ALL GATES PASS
DDRM_VERIFY_FAST=1 scripts/ddrm-verify.sh       # skip the heavy gates 3+4 (1+2 only)

# (the gate's three parts, runnable on their own)
scripts/ddrm-drift-check.sh                     # contract drift — expect PASS
scripts/pc2-conformance.sh                      # cross-impl parity — PASS (or SKIP without PC2)
scripts/ddrm-ladder-check.sh                    # ladder counts + wasm builds — expect INTACT

# per-provider host tests (fast, authoritative): 13+12+9+9+25 = 68 green, 0 ignored
for p in encrypt drm rights key decrypt; do (cd capsules/$p-provider && cargo test); done

# decrypt-provider feature ladder (tested islands; counts in §3)
# default 25 / rail-prep 27 / pq-envelope 29 / pq-rail-prep 31 / vectors 42 /
# rail-shim 45 / pq-mldsa 34 / rail-shim-mldsa 54 / harden 65
( cd capsules/decrypt-provider && \
  for f in rail-prep pq-envelope pq-rail-prep vectors rail-shim pq-mldsa rail-shim-mldsa harden; do \
    cargo test --features $f; done )

# regenerate the committed golden vectors (only when intentionally changing them)
( cd capsules/decrypt-provider && cargo test --features gen-vectors emit_ )
# (the ML-DSA goldens need pq-mldsa too:)
( cd capsules/decrypt-provider && cargo test --features "gen-vectors,pq-mldsa" emit_ )

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

## 12. Day log (1–28, one line each)

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
- **D25** refresh `HANDOVER.md` to current truth (Days 18–24) (`874c3f5b6`).
- **D26** encrypt→decrypt round-trip golden — both invariants on one artifact (`vectors`=37) (`48aef61c9`).
- **D27** rail transport shim behind `rail-shim` — `decrypt_from_carrier`, both profiles, un-wired (`f3d09e922`).
- **D28** pin the carrier as a portable golden + PC2 session-level conformance (`rail-shim`=43) (`363d75b09`).
- **D29** refresh `HANDOVER.md` to current truth (Days 25–28) (`80137f260`).
- **D30** PQ carrier golden through the shim — profile symmetry closed (`rail-shim`=45) (`e4e4d11c2`).
- **D31** widen cenc goldens to real shapes (multi-sample/subsample/init-IV) + PC2 parity (`vectors`=40) (`787bb3acd`).
- **D32** wire the real FIPS 204 ML-DSA-65 into the `CekSealVerifier` slot behind `pq-mldsa` (=34) (`d6899b9ed`).
- **D33** verify the real ML-DSA-65 through `decrypt_from_carrier` on a carrier golden (`rail-shim-mldsa`=54) (`aadb4f1fc`).
- **D34** adversarial negative-space + containment sweep behind `harden` (=65) (`b1f8b7dd5`).
- **D35** make the gate authoritative (`ddrm-ladder-check.sh`: counts + wasm) + handover refresh (`90899e70d`).
- **D36** reconcile-prep: widen drift guard to full consumed surface (fn + DEFAULT_* + PQ-algo fields), button-press rebase recipe, gate the encrypt↔decrypt seam by name (`d1035d98b`).
- **D37** widen the producer round-trip to real shapes: encrypt-provider emits multi-sample + subsample round-trip goldens, replayed byte-exact by decrypt (`vectors`=42); gate asserts all 3 seams by name (`c63c375db`).
- **D38** prove PC2 consumes the producer's output: drive the multi-sample + subsample producer segments through PC2's real `mp4box`+`cenc` (byte parity + wrong-CEK key-bound) in `pc2-conformance.sh` (`926b9adcb`).
- **D39** reconcile `encrypt-provider` to `elastos-common`: sealed output now the shared `SealedObjectV1`/`KeyEnvelopeV1` (typed), algorithm set checked by the shared validator; only input `SealRequest` stays local; Day-16 self-containment retired (`b3b5f0a9d`).
- **D40** integrity audit: every claim→gate mapped (table in `DDRM_STATUS.md`), no orphan vectors / dead flags, counts re-validated fresh; **WASI smoke wired into `ddrm-verify.sh` as gate 4/4** (skips clean w/o wasmtime) — the last doc-only claim is now gate-backed.

---

## 13. Next

The Days 18–34 options are **done**: rail-prep, encrypt keygen, PQ envelope, PQ data
path, golden vectors, executable conformance, encrypt→decrypt round-trip, the rail
transport shim + carrier goldens (both profiles), widened cenc goldens (real playback
shapes), the **real ML-DSA-65 primitive** wired + verified through the rail entrypoint,
the **adversarial harden sweep**, and an **authoritative verification gate**. The
`decrypt-provider` crypto core is now **feature-complete, PQ-proven, and hardened** —
there is little high-value *novel engineering* left on it before the rail lands.

The rail itself remains **blocked on Anders** (now a one-line `OpenSession` wire-up;
the signature primitive is no longer a build gap — only Anders' Q2 policy). The
next-day prompt is normally provided by the prior agent; if you do not have it, the
highest-value **unblocked** options, in order, are:

1. **Reconcile-prep for 0.4.0** — keep `ddrm-verify.sh` green; tighten the drift guard /
   `PUSH_PLAN.md` rebase recipe so the eventual rebase onto a stabilised 0.4.0 is a
   button-press. Highest leverage now that the core is done.
2. **Encrypt-side `seal` completion** — the encrypt `seal` (PQ-envelope CEK escrow + fMP4
   packaging + ciphertext availability) is still behind a fail-closed `seal`; it shares
   the decrypt rail dependency but its in-boundary packaging can be advanced + pinned with
   a round-trip golden against the decrypt side. See `DDRM_ENCRYPT_INVARIANT.md`.
3. **Crosvm macOS build hygiene** (orthogonal, optional) — `object-provider` fails to
   build on macOS via `elastos-crosvm` (`libc` `sockaddr_in.sin_len`); a small
   platform-gating fix would restore a green `cargo build` there. Not on the dDRM
   critical path (crosvm is the Linux/KVM substrate, not the live macOS path).
4. **Tighten the hybrid-signature transition** (de-risk Anders' Q2) — spike a hybrid
   `ECDSA + ml-dsa-65` `CekSealVerifier` so *both* answers to Q2 are pre-proven, leaving
   the rail landing a pure wiring step regardless of the policy chosen.

Whatever you pick: keep it isolated on `feat/decrypt-provider-cenc`, pin it with
characterization tests, keep the gate green (`scripts/ddrm-verify.sh` + the 68
provider tests), update `DDRM_STATUS.md`, and end the day by presenting the next
10/10 prompt.
