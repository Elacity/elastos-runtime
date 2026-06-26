# Flint shell — UX vision (the glass delegation desk)

The canonical product vision for the Flint shell (Phase 4, on `flint-shell`). Produced by the
design-council ideation swarm `w0r00ut7f` (5 perspectives → synthesis → 3-lens critique →
refine). Sits on the runtime shipped + audited on `flint`. This is the WHAT + the FEEL; the
first thin slice is the build target. See `ROADMAP.md` (status) + `project_flint-shell-ux-vision`
(memory).

## The paradigm — identity collapse
Flint is not a desktop you operate; it is a witness-stand you delegate from. The shift: today
YOU are the middleware (you decompose goals into clicks, integrate apps by hand, hold the audit
log in fading memory, trust software by vibes). The ElastOS runtime already dissolves all four —
the intent compiler decomposes, the carrier integrates, the ed25519 signed chain IS the audit
log, signatures + capabilities ARE trust. So there is no "me clicking" lane and a separate "AI
acting" lane: you and your agents are the same actor, same scoped authority, same gate, same
signed receipts. You stop being the glue; you become the principal who grants authority and
witnesses acts. This is true at the ENFORCEMENT layer (Principle 7: the carrier gate and the
HTTP gate converge on one capability decision), not the marketing layer.

## The whole product — 3 objects + 2 properties (deliberately not eight things)
- THE CONVERSATION — one center column; state a goal, the five-beat renders inline as cards
  (plan → gate → receipt). The verb. NOT a chatbot transcript: every turn is a typed contract
  you ratify.
- THE GATE — the one sacred object: the seal you pass authority through, physically HEAVIER for
  one-way doors (press-and-hold / passkey + a single red seam). The brand lives in this half-second.
- THE SIDEBAR — standing authority, never a launcher: identity (DID), Library (human nouns),
  live revocable capabilities (one-tap kill), the receipt ribbon. Felt sovereignty + kill-switch.
- TRUST-AS-MATERIAL — every capsule reference wears a skin computed from real crypto: solid
  (verified) / glass (content-addressed) / ghost (unsigned, extra gate friction). The picture
  cannot lie. Shippable in CSS today.
- SCOPE — System vs SelfOnly; how much of a capsule is exposed, which authority level you see.

## The four founder questions, answered
- Agentic chat? YES — an intent STREAM, not a chatbot. The plan is editable BEFORE consent (you
  steer by trimming, not re-prompting). v1 input = an affordance/command COMPOSER grounded in
  reachable capabilities (it cannot propose a hallucinated action); free-text NL is the deferred
  ai-backend capsule (Roadmap wedge 3), gated like any other.
- The sidebar? Standing authority + kill-switch; it carries the sovereign feeling even in a calm,
  all-reversible session, not only at the rare gate.
- OS apps in windows/spaces? YES, but launch-from-a-dock dies. A window OPENS as the RESULT of an
  intent ("play this"), wears an authority bar (not a titlebar), and closing its grant closes it.
  ONE window at a time in v1; tiling + the 3D city are earned later, not shipped.
- How things manifest? Every pixel is a read-only PROJECTION of a typed fact the trusted core
  already owns. The design job is projection, not invention.

## The feel — earned calm
Dark, still, low-density, mostly negative space; type + material do the work. Rich moments ONLY
at trust-material + the gate seal (rich because they mean something). Motion rationed to two beats
(the plan composing, the seal), plays once, then absolute stillness; zero idle animation. Linear's
restraint + a hardware-security-key's gravity. Magical in an ACCOUNTABLE way, never spooky.
Language: goal, plan, gate, seal, receipt, revoke (drop the cinematic words — membrane, altitude,
city, halo). The north star: on a CALM first session a user thinks "I finally understand what my
computer is allowed to do, and it is beautiful."

## What makes it 10/10
1. Honest manifestation = an un-fakeable moat. A clone on a normal LLM would have to LIE (fake
   material, fake receipt chain, fake gate), and the lie is structurally detectable.
2. One verb (the five-beat) + one sacred object (the friction-scaled gate = anti-automation-bias
   made tactile).
3. Identity collapse made VISIBLE — the AI agent hits the same gate and drops a "via MCP" receipt
   next to yours. The deepest claim becomes something you watch happen.

## The first thin slice (buildable on what is SHIPPED)
One loop, real + beautiful, on discover + the intent compiler + the signed chain. NO 3D city, NO
NL box, NO meter/Venice, NO Godot, NO unshipped links narrated as real.
1. Compose a goal via an affordance composer → a real StructuredIntent.
2. discover returns the real fail-closed candidate set (or honest Ambiguous), each wearing real
   trust material (run on a tree with trust roots so verified/metallic material can appear).
3. A plan card renders the real typed contract as SINGLE gated steps (resources, actions,
   reversibility verdict), editable before consent. NOT a woven typed pipeline — shipped outputs
   are still opaque (wedge 2 mechanism is real; typed outputs are a separate manifest task).
4. The gate: light tap for a reversible read; HEAVY (press-and-hold + red seam + plain-English
   one-way clause) for a key-release/decrypt-class step. The demo's wow moment.
5. On seal: one capability-token pulse → the result capsule opens as the ONE window with its
   authority bar → a signed receipt settles into the ribbon, chained + clickable to replay.
Pick an ACT that COMPLETES in the shipped tree (inspect affordance / content-open / rights-check);
full dDRM decrypt/render is fail-closed without an operator backend — scope the demo to the live
ela.city asset as an explicit showcase, or choose a completing op. MCP today is read/preview, so
the honest cross-actor beat is "an MCP agent previews a plan and hits the same gate"; act-over-MCP
is the next dependency, not yet a closed loop.

## Decisions only the founder can make (open)
1. NL: ship the grounded composer now (buildable today) vs couple v1 to the deferred ai-backend
   for a free-text prompt?
2. Hero ACT: accept decrypt is fail-closed + scope to the live ela.city asset, or pick a completing
   op (inspect / content-open / rights-check)?
3. The brave cut: confirm the 3D city is a FUTURE magnification, absent from Flint v1 (the whole
   minimalism depends on this holding).
4. Trust roots in first-run onboarding (so verified material shows immediately) vs a glass-and-ghost
   default until configured?
5. Capability downscoping (tap a chip to narrow a grant's scope): does the runtime support
   parameterized re-scope of a resolved plan today? Confirm before shipping as a demo beat — a
   possible second moat (steer-by-trimming).
6. Act-over-MCP for the demo (a genuine cross-actor seal) vs scope the v1 claim to preview-parity?

Source: ideation swarm `w0r00ut7f`. Critique verdicts: minimal STRONG, paradigm STRONG, grounded
NEEDS-SHARPENING (sharpenings folded in: NL deferred, opaque outputs = no woven pipeline, trust
roots needed for verified material, MCP read/preview-only, decrypt fail-closed without a backend).
