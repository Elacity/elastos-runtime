# Next strategy — runtime-first, the ESP shell protocol, and the wedges

From council swarm `wgqlt5u74` (5 strategists audited the live `elastos-runtime` tree at HEAD
`97bcd3689` + a 6-visionary narrative panel). Answers: shell-vs-runtime, the modular pluggable-shell
architecture, the shell protocol, the current shell, the toggle, and the concrete wedges. Builds on
`PDR_SOVEREIGN_COMPUTER.md`. The narrative lives in `NARRATIVE.md`.

## The decisive recommendation
**RUNTIME FIRST — but a small bounded slice, with a THIN reference shell built IN PARALLEL as the
forcing function.** Not a hedge. A shell can only PROJECT what the core can PROVE and only ENFORCE
consent the core can GATE. Two of the three pillars the shell's value rests on are unbacked in tree
today, so a polished shell now would render consent the runtime refuses and a halo the capsule grades
itself on — the faked-autonomy anti-pattern we sell against, multiplied once a marketplace exists.
Do the three core fixes first; build only the thin reference client alongside them, to define the
protocol by usage.

## Is the shell still valuable? Yes — but only the THIN reference shell
The valuable thing now is a SECOND consumer of the bridge that forces a stable contract out of the
currently-hardcoded `/api/apps/home/*` gateway. It earns its keep three ways: (a) hardens/de-ad-hocs
what `shell-core.js` already does; (b) DEFINES the protocol by extraction, not committee; (c) building
it in a DIFFERENT framework (Svelte/TS vs the current vanilla JS) is the proof-of-work that the
shell-in-any-language boundary is a real wire protocol, not a linker dependency. The rich shell (full
Grant Garden, weighted seal, the Review, the 320ms toggle as polished product) is DEFERRED until the
three fixes land. A shell on stubs is the anti-pattern with better art.

## The modular-shell verdict (the highest-leverage idea in the brief)
Architecturally RIGHT and the platform prize — but it's a **PROTOCOL bet, not a SHELL bet**, and the
founder's framing carries one inversion to correct:
- WRONG: "everything connects to the current shell and we swap it later."
- RIGHT: **nothing connects to a shell; everything connects to the RUNTIME.** Shells are never the
  integration point; the neutral runtime substrate is. You don't swap shells — you run both as
  untrusted clients of the same core and toggle focus.

Precedent: **LSP** (a capability-projection protocol where the server owns all ground truth and the
client is a dumb-but-pretty renderer) + **Wayland** (the compositor owns input/focus/authority;
GNOME/KDE/Sway are "shells in any language" over one protocol). The browser is a WEAKER analogy (each
browser ships its own monolithic engine) and must not anchor the design — our model is one engine
(core), many clients. **The moat no precedent had:** the triple-duty capability token (consent =
trade = audit). A third-party shell that wants to ACT must present a token the CORE minted and
re-checks, so a marketplace of UNTRUSTED shells is structurally SAFE for us in a way browser
extensions never were. WHEN: define the protocol NOW via the first second-shell; build the
marketplace LATER (2027+), gated on ESP v1 surviving your own second client without a breaking change
AND a real external party who WANTS a bespoke shell. Design as if the marketplace will exist; ship
only your own shell against it.

## ESP — the ElastOS Shell Protocol (v0)
A versioned, language-agnostic, capability-gated contract over the existing local bridge, defined by
EXTRACTION from what `/api/apps/home/*` already half-implements. TWO PLANES and ONE INVIOLABLE RULE.

**THE INVIOLABLE RULE (the constitution):** the gate is in CORE, never in the protocol and never in
the shell. The shell is UNTRUSTED, READ-ONLY-BY-DEFAULT. Token check, risk classification, egress
check, and receipt write all happen on the far side of the bridge where the shell has no reach. A
malicious shell can render the gate as a lie but cannot OPEN it. Corollary baked into the schema:
**every projected fact carries the receipt id that backs it; the shell MUST refuse to render any
object lacking one — an unbacked tile is a PROTOCOL VIOLATION, not a cosmetic bug.**

**PLANE 1 — PROJECTION (core → shell, read-only, push):** a typed append-only stream of runtime FACTS
over HTTP snapshots + SSE deltas (the `events/stream` endpoint already ships). Fact families = the
product vocabulary made typed:
- `CapsuleState{id, tier(wasmtime|crosvm), lifecycle, declared_affordances, DERIVED_reach}`
- `CapabilityGrant{token_id, scope, action, ttl_expiry, revoked, delegatable}` (the Grant Garden)
- `ReceiptAppended{seq, prev_hash, hash, kind, signer, dual:[user_proof, platform_attestation]}` (the
  Flight Recorder + Dual Receipt, drives the Review)
- `ReachFact{capsule_id, declared, observed, exit_provider, allowed_hosts}` (the halo — `declared`
  labeled UNVERIFIED and `observed` empty until egress-as-capability lands, so the protocol FORCES
  the halo to show degraded until it's true)
- `ConsentRequest{request_id, capsule, method, risk, human_summary, proposed_constraints}`

**PLANE 2 — CONSENT/ACT (shell → core, request-only, the ONLY write path):** exactly three verbs —
`request(scope)` → a pending capability request; `approve(request_id)`/`deny` → the consent decision;
`invoke(token, method, args)` → the act. Core re-resolves the affordance, re-checks token
scope/ttl/revocation AND reach policy, mints/spends the grant, performs the act, emits the dual
receipt back over Plane 1. The shell observes the result; it NEVER performs the effect, NEVER holds a
key (dKMS is 2-of-3 Shamir behind the bridge), NEVER mints a token.

**VERSIONING:** LSP-style `initialize` handshake; additive fields never break; unknown fields MUST be
ignored (the HTML ignore-unknown-tags discipline); removals bump major; each stream independently
semver'd. **TRANSPORT:** HTTP + SSE today; the fact schema is the contract, the wire is swappable; add
websocket later only as a latency optimization for the toggle. **SANDBOX:** run each shell AS a capsule
behind the same five-beat gate (wasmtime for web/TS), scoped to subscribe-projection + the three
verbs, preopened dir only, default-no-internet; a marketplace shell needs an explicit reach-tagged
grant the user SEES in the halo. **PUBLISHED ARTIFACT:** `docs/ESP_V0.md` (JSON-Schema) + a shared TS
type package the shell imports — that schema, not the code, is what others build against. **CONFORMANCE
(next, not now):** a golden recorded receipt-chain any shell must render exactly and against which any
shell can drive ZERO effects without a core-minted token. That harness IS the future marketplace
admission gate.

## The current shell (confirmed from tree)
`capsules/home` — a WASM capsule with `capsule.json` role `shell` (type wasm, entrypoint `home.wasm`)
whose user-facing surface is the plain browser bundle in `capsules/home/browser/` (`shell-core.js`,
`shell-windows.js`, `shell-surface.js`, `shell-window-geometry.js`, `shell-chrome.js`, `shell-auth.js`,
`shell.js`, `index.html`). **VANILLA ES-module JavaScript** — no package.json, no Svelte/React, no
build step — talking to core ONLY over HTTP `/api/*` + an EventSource SSE stream. A genuine windowed
desktop (launcher/taskbar/windows/snapping), correctly OUTSIDE the trusted core. Both PDR claims hold:
"Home is already browser-hosted/web" and "the shell must be web, not Rust-native; Godot rejected."
**Keep it** as the OS-lens / front door / reference ESP client. **Build the NEW agent shell as a SECOND
ESP client in TS/Svelte** (different framework = proof of language-agnosticism). HYGIENE: there's also
a HEADLESS capsule literally named `shell` (the supervisor special-cases the string for token issuance)
that is a decision/auto-grant engine — rename it `consent-broker` before it collides with the UI-shell
and marketplace vocabulary.

## The toggle
Trivially real and conflict-free ONCE ESP exists, because both surfaces are ESP clients over the SAME
bridge to the SAME runtime + capsules + receipt log. No conflict because neither holds authority — the
core does. The toggle does NOT navigate/reboot/reload: the SAME desktop substrate stays mounted and
only the LENS changes (~320ms is pure material-refraction, zero state reconciliation; a token granted
in the agent lens is already live in the OS lens by construction). The founder's "button out of the
shell" = a runtime-owned SHELL PICKER (the session/desktop-environment-chooser pattern) listing the two
shells now with a "browse shells" rail = the future marketplace seam. Backend = ONE de-hardcode:
generalize the supervisor's magic-string `shell` token-issuance special-case to "any capsule of
`CapsuleRole::Shell` passing `is_shell_launchable`, selected by a user-set active-shell pointer." When a
user swaps to a third-party shell later, show a one-time TRUST CARD honest by design: "This shell can
SEE everything you can see and ASK on your behalf. It can do NOTHING you do not approve, and every act
it requests is signed into your receipt log, which this shell cannot edit." True because of the
inviolable rule.

## The wedges + tasks NOW (sequenced, on the shipped tree)
- **W0 (Rust, core; gates the honest halo):** make reach a CORE-DERIVED fact, not a manifest claim.
  Add a typed reach descriptor the runtime DERIVES from the capsule's actual granted net capability
  (tier + whether an Exit Provider is bound); stop trusting self-declared `AffordanceRisk`
  (`elastos-common/src/manifest.rs:199`, field at :148) as ground truth for reach. Keep DECLARED risk
  as advisory; core stamps an OBSERVED reach. Do NOT add reach as a self-declared variant.
- **W1 (Rust, core):** egress-as-capability. Add `RuleCheck::ReachAllowlist{allowed_hosts/schemes}` to
  `capability/policy.rs:357` and ENFORCE at the wasmtime/crosvm net-provider / Exit-Provider boundary,
  so default-no-internet means no socket unless a reach-scoped expiring token says so. This is the
  substance behind the EU AI Act Art-14 enterprise pitch; until it lands the halo is decorative.
- **W2 (Rust, core; THE single highest-value task):** unstub the consent ACT path. Replace the flat
  `StatusCode::FORBIDDEN` "user-approved affordance invocation is not enabled yet"
  (`gateway_capsule_catalog.rs:332-336`) with a real round-trip: `AffordanceApprovalMode::User` emits a
  `ConsentRequest` onto `events/stream`, accepts an `approve(request_id)` verb, mints a scoped/expiring
  ed25519 grant, dispatches the act, emits the dual receipt. Route Payment/Rights/Actuator/Privileged
  through the same flow. Update the test at :1102. The hero gesture is dead without this.
- **W3 (Rust de-hardcode, tiny but pivotal):** generalize the supervisor's magic-string `shell`
  token-issuance to "any `CapsuleRole::Shell` selected by a user-set active-shell pointer" (the entire
  backend of the shell-picker). ALSO rename the headless `shell` decision-engine to `consent-broker`.
- **W4 (protocol, ALONGSIDE W0-W2, by extraction):** write ESP v0 as `docs/ESP_V0.md` (JSON-Schema) +
  a shared TS type package — the five projection fact families, the three verbs, the initialize
  handshake, ignore-unknown-fields, per-stream semver, transport-independence. Mark `ReachFact.observed`
  and User-approval `invoke` as currently-degraded until W0/W1/W2 land. Then refactor `shell-core.js` to
  consume these facts over the SSE projection endpoint instead of bespoke `/api` calls.
- **W5 (the v1 shell that proves the thesis WITHOUT the marketplace):** build ONE new TS/Svelte
  projection client (a DIFFERENT framework, as proof of language-agnosticism) consuming ESP v0
  read-only and rendering exactly: the live Grant Garden, the weighted keystone seal for the focused
  capsule, the halo (degraded where W0/W1 not yet landed), and ONE hero dDRM act
  (open-rights-checked-decrypt, keys-used-not-owned) flowing perceive→plan→consent→act→audit through
  W2 and emitting the DUAL RECEIPT onto the hash-chained log. Run it AS a sandboxed wasmtime capsule
  (no socket, preopened dir). Done when it renders a real dual receipt and renders NOTHING without one.
- **W6 (the toggle seam, cheap):** wire the REFRACTION TOGGLE between vanilla-JS Home (OS lens) and the
  new TS agent shell (agent lens) as a focus-swap over identical projected state (~320ms, one source of
  authority, no state migration). Add the runtime-owned shell-picker sheet (hardcoded list now) + the
  honest Trust Card. Delivers "both available, switchable, no conflict" AND prototypes the marketplace
  mechanic at near-zero cost.
- **W7 (flywheel, once):** wire W5's hero dDRM dual receipt to the enterprise wedge — export the
  receipt chain as the EU AI Act Art-12/14 / SOC2-for-agents containment-audit artifact. The SAME
  receipt that delights the consumer IS the compliance evidence. The flywheel's first turn.

## What NOT to do yet
- The MARKETPLACE OF SHELLS (browse/install/publish/registry/revenue-share): 2027+, gated on ESP v1
  surviving a second client + a real external design partner. A marketplace over a half-trusted gate is
  the anti-pattern at ecosystem scale.
- The RICH/polished agent shell before W0/W1/W2 land.
- Letting the shell hold ANY authority (no optimistic grants, no mock receipts, no shell-side token
  minting / risk classification / egress check / key handling). The shell's entire write surface is the
  three verbs.
- A websocket transport upgrade as a blocker (HTTP+SSE is correct now; the wire is swappable).
- A Rust-native / Godot / runtime-linked shell (re-couples shell to core, kills language-agnosticism).
- Over-freezing ESP as a public standard before the second consumer exists (LSP earned generality by
  serving many editors; generality is discovered, not declared).
- Rendering any halo as trustworthy before W0/W1 (mark reach declared/UNVERIFIED, show degraded).
- Marketing the modular-shell ecosystem as a CURRENT capability (sell runtime + containment now; tease
  portability via the two-shell toggle as PROOF, not a storefront).

## Open decisions for the founder
1. SEQUENCING: confirm W2 first (highest value), then W0/W1, with W4 alongside; hold the rich shell
   until all three land.
2. REACH MODEL: confirm core DERIVES observed reach (declared risk advisory-only; no self-declared
   reach variant).
3. RENAME: approve renaming the headless `shell` decision-engine to `consent-broker`.
4. SECOND FRAMEWORK: pick the new agent shell's framework (default Svelte — lighter, compiles away,
   suits a sandboxed wasmtime capsule).
5. HERO ACT: confirm open-rights-checked-decrypt (keys-used-not-owned) as the first hero act, or name
   the specific dDRM asset/flow.
6. MARKETPLACE TIMING: agree 2027+, gated on the two conditions.
7. PUBLIC NARRATIVE BOUNDARY: market runtime + containment + the two-shell toggle as PROOF now; not a
   marketplace or a confident halo until W0/W1 land and the protocol survives its second consumer.
8. ENTERPRISE WEDGE OWNERSHIP: decide who owns the first design-partner conversation and whether the
   audit-export format is co-designed with a regulator-facing partner before W5 ships.

Source: swarm `wgqlt5u74`. The narrative: `NARRATIVE.md`.
