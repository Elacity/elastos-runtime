# Protected-content journey implementation plan (mint → sell → buy → play)

Status: proposal. This plan describes how to take the canonical protected-content
extraction (`feat/protected-content-*`) from a trust-core foundation to the full
end-to-end user journey — **mint → sell → buy → play** — with first-class design and
UI/UX, coherent with [PRINCIPLES.md](../PRINCIPLES.md) and the canonical architecture in
[PROTECTED_CONTENT.md](PROTECTED_CONTENT.md).

It is written to complement, not replace, the "PR #15 disposition" already recorded in
[PROTECTED_CONTENT.md](PROTECTED_CONTENT.md): PR #15 (`feat/dkms-esp-port`) is research and
behavior evidence; this plan is how the canonical line reaches the same user-visible
capability on the ESP substrate without inheriting PR #15's rejected patterns.

---

## 1. Goal and definition of done

One sentence: **a signed-in person can seal an asset in Create, list it in the Store, a
second person can buy it with an explicit spend confirmation, and open and play it from
their Library — and at no point does a capsule, the marketplace, or the viewer hold raw
key material, wallet authority, or chain authority.**

"Done" for the 0.7 window is a real vertical slice, not the whole catalog of features:

- one content asset type (media), one custody topology (the source 2-of-3 pool, one-node
  provider process acceptable for the dev lane), one viewer (`elastos.viewer/media@1`);
- mint, list, buy, acquire, open, and play all reachable from Home by an ordinary user;
- a green end-to-end acceptance oracle proving the whole rail;
- every unauthorized step fails closed with an explanation.

Everything past that (apps/games as sellable assets, resale/trade, multi-segment media,
DKG/rotation/revocation, PQ-hybrid custody) is explicitly deferred and named in §9.

---

## 2. Vision alignment (this is the coherence contract)

Every stage below is designed against [PRINCIPLES.md](../PRINCIPLES.md). The mapping is the
acceptance criterion for "aligned with the ElastOS vision", not decoration:

| Principle | How this journey honors it |
|---|---|
| 1 Local first | The bought asset is a normal object pinned into the buyer's `localhost://` Library; discovery and playback are explained as Library/Store objects, not host URLs. |
| 3 No ambient authority | Capsules request typed Runtime resources only. Marketplace, Create, and viewers never hold a signer, CEK, IV, RPC, or provider route. |
| 4 Carrier is not the capsule contract | Custody nodes are reached over Carrier *behind* Runtime resources; capsules never name node endpoints. |
| 5 Small trusted core | Authority, orchestration, audit live in Runtime; crypto lives in the canonical crates + providers; UI lives in capsules. No app reimplements decryption or license policy (15). |
| 10 One canonical path / 11 Fail closed | One `open` gate, one `buy` verb, one `mint` verb. Missing rights → 403 → Home offers buy; missing custody → explicit error, never a weaker fallback. |
| 13/14 Objects, capsules, spaces distinct; human names | Product surfaces read **Create**, **Store**, **Library**, **Wallet** — not dKMS/dDRM/keygen. Internal terms stay in developer docs. |
| 15 Trust travels with signed content | Viewer selection comes from the sealed object's `viewer.required_interface`; access policy and CEK commitment are bound into signed metadata. |
| 16 UI surfaces are not authority | Every buy/open/mint requires the capability for that surface. Home binds browser-frame messages to the launched frame + capsule-scoped capability before acting; a launch token in a URL fragment is not authority by itself. |
| 17 Design tokens are product contracts | All surfaces use `capsules/_shared` tokens; money verbs get one consistent, unmistakable visual treatment across the whole journey. |
| 7 Humans and agents share one authority model | The buy/open/mint operations an agent would call are the same capability-scoped operations the UI triggers; no UI-only side door. |

If a design choice cannot be expressed as one of the rows above, it is out of scope or
wrong.

---

## 3. Architecture spine (reused, not reinvented)

The canonical path from [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md) is the backbone for
all four stages:

```
capsule  ->  Runtime coordinator  ->  rights-provider  ->  custody providers  ->  decrypt-provider
```

- **Runtime** derives authority from the authenticated Profile, exact Wallet approval,
  session, object, and action; selects providers; owns durable orchestration and audit.
- **rights-provider** evaluates the exact approved policy through typed Chain evidence; it
  does not release keys.
- **custody providers** each independently verify the exact Runtime operation and rights
  evidence and return only a recipient-encrypted contribution + authenticated receipt.
- **decrypt-provider** is the only boundary that reconstructs and briefly holds a live CEK,
  returns scoped output, and zeroizes.
- **Carrier** transports Runtime-selected endpoint traffic; it is not rights/custody/key
  authority.

Canonical crates already in the extraction (reuse as-is; extend at their boundaries):
`elastos-protected-content-contracts`, `elastos-protected-content-custody`,
`elastos-protected-content-provider-contracts`, and the `capsules/custody-provider`
process.

---

## 4. The journey, stage by stage

Each stage lists the **user story**, the **UI surface**, the **Runtime seam**, the
**providers**, the **authority/security**, and the **object model**. Reuse/re-derive/reject
decisions vs PR #15 are consolidated in §8.

### 4.1 Mint — "Create and seal an asset"

- **User story:** a creator picks a file in **Create**, sets access terms (free / buy-once
  / buy-and-resell) and price, and publishes. One action seals + escrows + lists (sale
  terms are baked into the mint, so primary listing == minting).
- **UI surface:** `capsules/creator` (adopt from PR #15, harden). Single "Create asset"
  flow: choose object from Library → choose viewer interface (auto from media type) → set
  terms → review → **mint (money verb)**. Poster/cover is chosen here; no secure-preview
  egress ever (asset detail later shows poster only).
- **Runtime seam:** `POST /api/create/mint` (`mint_authority`) — a money verb (§6).
- **Providers/crypto:** CEK minted inside the encrypt/custody boundary; CENC-encrypt;
  content-address the ciphertext; escrow CEK shares to the custody pool at publish time via
  the canonical `custody-provisioning` + `elastos-protected-content-custody` provisioning.
  Public metadata carries only bounded identities, threshold/epoch/pool facts, CEK
  commitment, and signatures — never shares.
- **Object model:** the sealed object binds the exact pool identity, epoch identity, and
  committee-authorization identity it was minted against; `viewer.required_interface` is
  written into the object so Runtime can later pick the viewer without guessing.

### 4.2 Sell — "List it in the Store"

- **User story:** the minted asset appears in the unified **Store** for others to discover;
  the creator can see/manage their listings.
- **UI surface:** one canonical **Store** (consolidate `capsules/marketplace` and
  `capsules/marketplace-content` into a single asset-type-aware surface; fold the existing
  app-store in as the "Apps" fulfillment path later). The Store **mints nothing, plays
  nothing, holds no signer/CEK/RPC** — it only decodes listings and produces (a) an unsigned
  buy intent and (b) a pinned encrypted file on fulfillment.
- **Runtime seam:** read side of `/api/market/*` (listing/index decode over
  `chain-provider` getLogs cache). Marketplace uses the `content/*` plane, never
  `ipfs-provider` directly.
- **Authority/security:** listing reads are non-money; no signer. Asset detail shows
  poster/cover only.

### 4.3 Buy — "Acquire the access right"

- **User story:** a buyer opens asset detail, taps **Buy**, sees exactly what they are
  about to sign and spend, confirms with a passkey, and the encrypted file lands in their
  Library.
- **UI surface:** Store asset detail → **Buy** → **Home spend-confirmation dialog**
  (`capsules/home/browser/home-spend-prompt.js`, adopt + harden): *what is displayed is what
  is signed*. Rendered in Home chrome, not inside the app frame (16 UI-is-not-authority).
- **Runtime seam:** `POST /api/market/buy` (`buy_authority`) — money verb. Re-verifies the
  listing live and aborts on drift; produces an **unsigned** `buyAccess` for the wallet to
  sign and broadcast. Fulfillment: `object-provider Acquire` pins the encrypted CID into the
  buyer's Library.
- **Authority/security:** money verb (§6) — passkey step-up + Wallet approval; the CEK is
  never touched at buy time (buying is an on-chain access right, not a key handover).

### 4.4 Play — "Open and play from the Library"

- **User story:** the buyer opens the asset from their Library; it plays; the key is never
  exposed; when they close it, the session and any live key material are gone.
- **UI surface:** Runtime-selected viewer capsule — `elacity-player` for
  `elastos.viewer/media@1`, `ddrm-viewer` for `elastos.viewer/document@1` (adopt + harden).
  The Store and other apps never choose a viewer.
- **Runtime seam:** `POST /api/viewers/open { uri }` is the single open gate:
  1. resolve the object inside the principal's own root only;
  2. resolve the on-chain subject (the parked `RequiredHomeLaunchToken` rewire — §6);
  3. ask `rights-provider → chain-provider has_access_by_content_id`, get a signed rights
     receipt;
  4. launch authority bound to `content_id` + rights-receipt hash (welded into the decrypt
     transcript AAD);
  5. return `{ viewer, session, title, play_url, rights_binding }`.
  A rights-denied open returns 403; Home offers the buy and retries (one canonical retry
  path). A non-owned and a non-existent asset return **byte-identical 404** — the refusal is
  not an existence oracle.
- **Session lifecycle:** open → session-bound scoped reads → close → sweeper reap. The
  decrypt boundary reconstructs the CEK only inside the scoped session and zeroizes on close
  or expiry. The viewer receives scoped output only (rendered / stream / working-copy),
  never key material.

---

## 5. UI/UX design (primary focus)

### 5.1 Design system and consistency

- All journey surfaces consume `capsules/_shared/{elastos-theme.js, elastos-ui.css,
  elastos-accent-picker.js, fonts}`. No one-off color literals (17). Colors are named by
  role; Home owns the wallpaper and brand layer.
- Each functional surface may use a scoped accent palette (Store, Create, Viewer), but the
  same role token means the same thing everywhere (a "destructive"/"spend" token is one
  color across Home, Store, and the spend prompt).
- Windows use the canonical `home-gui` chrome modes
  (`capsules/home-gui/browser/{home-gui.js, shell-core.js, style.css}`). Create, Store, and
  viewers are ordinary App windows; the spend prompt is Home chrome, never app chrome.

### 5.2 Information architecture (human nouns, principle 14)

- **Create** — seal + list (one app; primary listing == minting).
- **Store** — one canonical discover/buy/manage surface (consolidated). Asset-type aware;
  content today, apps/games later via the existing install path.
- **Library** — where bought objects live and are opened; protected assets are normal
  objects with a lock affordance and a Store provenance chip.
- **Wallet** — signer and balances; surfaced only through Runtime-mediated approval, never
  embedded in the Store.
- **Viewer** — Runtime-selected; full-bleed playback with a minimal chrome and a visible
  session/expiry indicator.
- Money verbs are **never** silent: mint and buy always route through the Home
  spend-confirmation dialog.

### 5.3 Key flows (wireframe-level)

1. **Discover → detail:** Store grid → asset detail (poster/cover only, terms, price,
   creator identity chip). No secure preview, no decrypt at detail.
2. **Buy:** detail **Buy** → Home spend-prompt (asset, price, chain, destination, the exact
   `buyAccess` being signed) → passkey step-up → progress → "Added to your Library" with a
   deep link to open.
3. **Open/play:** Library item **Open** → (if owned) viewer launches and plays; (if not
   owned) 403 → inline "You don't own this yet — Buy?" → buy flow → auto-retry open.
4. **Close:** explicit close or expiry → session ends, key zeroized, viewer shows a clean
   "session ended" state; reopening re-runs the open gate.

### 5.4 States and copy (fail closed, then explain — principle 11)

Every surface defines: loading, empty, success, rights-denied (403 → offer buy),
custody-unavailable (explicit, no fallback), session-expired, chain/wallet-declined,
and offline. Copy explains *what is missing and what the correct path is*; no spinner that
hides a fail-closed state. Error tone is calm and specific, consistent with Home/System.

### 5.5 Accessibility and trust cues

Money verbs and the spend prompt meet the highest contrast/focus/confirmation bar in the
product: keyboard-first, explicit confirm, no defaulted destructive focus, and a visible
"what is signed" region that is literally the signed payload. The lock/ownership state on a
Library object is conveyed by more than color.

---

## 6. Authority and security design (the hard core)

- **Money verbs** (`/api/create/mint`, `/api/market/buy`) require passkey step-up that is
  intent-, launch-, and time-bound and single-use, brokered under Home's cookie authority,
  and rendered by the Home spend-confirmation dialog (what is displayed is what is signed).
  Launch tokens travel in URL fragments, never query strings.
- **Subject-resolution rewire (the PR #15 parked blocker):** thread
  `RequiredHomeLaunchToken` through `viewer_open` and the `gateway_marketplace` read sites
  so `resolve_subject_address` can mint a `RuntimeWalletAuthority` and enumerate the
  principal's linked EVM account. Until this lands, chain-gated open/buy is dev-lane only
  and must say so (11). **This is scheduled in Phase 2, not deferred**, because it is the
  actual gate between the dev lane and a real buy.
- **Key custody invariants (already canonical):** CEK never whole outside a sandbox; one
  node-sealed share per node; recipient-sealed contributions (RFC 9180 X25519 + HKDF-SHA256
  + AES-256-GCM); exact-threshold reconstruction (required-plus-one rejected); CEK
  commitment checked after reconstruction; zeroization; audit.
- **Refusal is not an oracle:** non-owned and non-existent objects are byte-identical 404s.
- **No second authority path:** the provisional `drm/rights/key/decrypt` providers and
  `elastos_common::protected_content` DTOs are removed atomically at cutover (§7 Phase 5),
  never kept as a parallel decoder.

---

## 7. Workstreams and sequencing

Difficulty is described by scope and risk, not calendar time. Phases are ordered by
dependency; the 0.7 target is Phases 0–3 plus the Phase 4 minimum.

**Phase 0 — decisions and scaffolding (unblocks everything).**
- Stand up a single 0.7 integration branch that composes collaboration + this journey.
- Adopt PR #15's `dkms_rail` e2e as a *behavioral oracle* on the canonical line (start red;
  it defines "done").
- Converge naming: product nouns Create/Store/Library/Wallet/Viewer; internal terms
  protected-content/custody/rights/decrypt; retire dKMS/dDRM from user-facing copy.
- Fix the `custody-provider` process-test clock coupling (inject a clock or widen windows)
  so the one runnable piece is CI-stable.

**Phase 1 — dev-lane vertical slice (proves the contracts compose).**
- Wire Runtime orchestration end to end for one media asset with `ELASTOS_DDRM_SUBJECT` dev
  subject: mint → list → buy(dev) → acquire → open → reconstruct (one-node custody) → play.
- Minimal UI: Create (mint), Store (list/detail/buy), Library (open), viewer (play).
- The `dkms_rail` oracle goes green in the dev lane.

**Phase 2 — real chain-gated buy/open.**
- Land the subject-resolution rewire (`RequiredHomeLaunchToken` threading).
- Real Wallet + Chain integration for `buy`/`mint`; Home spend-confirmation dialog +
  passkey step-up (money-verb authority).
- rights-provider evaluates live chain evidence; open gate uses real subject resolution.

**Phase 3 — viewer session hardening.**
- Full open → session → close → sweeper lifecycle; expiry; zeroization proofs; media and
  document viewers; scoped-output-only enforcement; refusal-not-oracle test.

**Phase 4 — UI/UX polish and consolidation.**
- Consolidate to one canonical Store; Create app polish; Library lock/provenance chips;
  full design-token pass; all states/copy per §5.4; accessibility bar for money verbs.

**Phase 5 — atomic cutover and release gates.**
- Remove the provisional providers + DTOs atomically; align docs/state/tests (12);
  entropy/WCI/verify gates; manual browser walkthrough of the whole journey.

**Deferred past 0.7 (named, fail-closed):** resale/trade verbs, apps/games as sellable
assets, multi-segment media, DKG/rotation/re-share/revocation, PQ-hybrid custody,
multi-node production custody topology.

---

## 8. Disposition vs PR #15 (reuse / re-derive / reject)

Consistent with the "PR #15 disposition" in [PROTECTED_CONTENT.md](PROTECTED_CONTENT.md):

- **Adopt and harden (UI/flows, re-anchored on canonical authority):** `creator`,
  consolidated `marketplace`/`marketplace-content` → Store, `elacity-player`,
  `ddrm-viewer`, `home-spend-prompt.js`, the `/api/market/*` and `/api/viewers/*` seams, and
  the `dkms_rail` e2e as an oracle.
- **Re-derive at the canonical boundary (do not port wholesale):** custody durable shard
  storage, rights evaluation, decrypt boundary, provider roles, Runtime open scenarios —
  built on the canonical crates, not PR #15's internals.
- **Reject from the product path:** public aggregated `shares[]` metadata, capsule-owned
  authority, raw-CEK operations, `rail_shim`/reference fallbacks, old `drm-provider`
  orchestration, direct TCP/IP topology in capsules/contracts, and the standalone harness as
  a product route. PR #15's `escrow.json` fixture is not canonical metadata.

---

## 9. Testing and acceptance

- **Behavioral oracle:** `dkms_rail`-style e2e proving `mint → Runtime authority/selection/
  audit → per-node custody release → reconstruction/decryption → play`, in the dev lane
  first, then chain-gated.
- **Per-stage:** mint seal/commitment, listing decode, buy money-verb authority + drift
  abort, acquire pin, open gate (owned/denied/nonexistent byte-identical 404), session
  lifecycle + zeroization.
- **Negative/fail-closed:** tamper/expiry/wrong-binding rejection, required-plus-one
  rejection, custody-unavailable explicit failure, refusal-not-oracle.
- **UI:** manual walkthrough of discover → buy (spend prompt + passkey) → play → close, plus
  each defined state.
- **Stability:** clock-injected custody-provider process tests green on the CI runner class;
  no wall-clock-coupled flakes.

---

## 10. Risks and open decisions

- **Scope risk:** the journey's product surface (commerce + viewer + Runtime orchestration)
  is large relative to the extracted foundation; Phase 1's dev-lane slice is the de-risking
  gate — if it is not green, do not add custody depth.
- **Blocker risk:** the subject-resolution rewire is cross-module; if it slips, 0.7 must
  ship dev-lane buy explicitly labeled, not a silent half-path (11).
- **Consolidation decision:** one canonical Store (fold app-store in as fulfillment) must be
  committed to early to avoid two storefronts (10).
- **Naming decision:** lock the product vocabulary in Phase 0 so docs, code, tests, and UI
  agree (12/14).

---

## 11. Coherence checklist (merge gate)

- [ ] Every capsule in the journey requests only typed Runtime resources (3, 18).
- [ ] No capsule holds a signer, CEK, IV, RPC, or provider route (3, 15).
- [ ] One open gate, one buy verb, one mint verb; no fallbacks (10, 11).
- [ ] Money verbs always render the Home spend-confirmation dialog; what is displayed is
      what is signed (16).
- [ ] Product copy uses human nouns; dKMS/dDRM stay in developer docs (14).
- [ ] All surfaces use `_shared` design tokens and canonical window chrome (17).
- [ ] Refusal is not an existence oracle; unauthorized steps fail closed with explanation
      (11).
- [ ] Provisional providers removed atomically; docs/code/tests/state agree (10, 12).
- [ ] The `dkms_rail` behavioral oracle is green.
