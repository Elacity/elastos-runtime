# Inspector → roadmap: where this branch is heading

What the Capsule Inspector work on `feat/capsule-inspector` is a foundation *for*.
This is direction, not a commitment — it records the next steps we want to work
toward so the intent behind the substrate isn't lost. Pair with
`CAPSULE_INSPECTOR.md` (what's built), `KNOWN_GAPS.md` (what's not yet asserted),
and `INSPECT_DDRM_MERGE_NOTES.md` (cross-branch).

## Where we are (the substrate already built here)

A read-only, capability-gated, object-centred view of every capsule — identity,
powers, provenance, audit — plus a metadata-driven **gate preview** that shows the
exact capability tuple (resources + actions + audit events) an operation *would*
require, before anything runs. On the real product path (`ProviderRegistry`),
verified through both transports (browser gateway + capsule carrier bridge), with
honest provenance and attested audit. Fail-closed throughout.

In one line: **we made the runtime's authority legible and previewable.**

## North star: complete the loop, then put a shell on top

The substrate is four-fifths of a control loop:

> **reflect → preview → approve → act → audit**

- **reflect / preview** — built (this branch).
- **approve** — next (the human-in-the-loop consent step).
- **act** — capability-gated dispatch (merge-gated on DDRM's `required_action_for`).
- **audit** — attestation surfaced (this branch); deepens as `act` lands.

On top of that loop sits the real prize: **selectable shells**, including an
**intent-led AI shell** where a user states an outcome and a *contained* agent
manifests it through the same capability-gated path — never outside it.

## Roadmap (ordered; parallel-safe unless noted)

1. **Approval loop** *(parallel-safe — next).* A fail-closed approval-intent core
   (mirroring `inspect`/`invoke`): given a previewed gate + requester scope, yield
   `Approved | Denied | PendingApproval`, defaulting to deny. A recorded, audited
   approve/deny decision carrying no token or signature material (#16). Closes
   `KNOWN_GAPS` G4.
2. **Invoke dispatch — the "act"** *(merge-gated on DDRM).* Built to consult DDRM's
   `required_action_for` so preview and enforcement agree by construction. Closes
   `KNOWN_GAPS` G3. This is what turns "preview" into safe, gated, audited action.
3. **Selectable shells + a shell-manager** *(net-new).* The runtime already routes
   everything through one capability-gated path and treats "the shell" as a
   capability-scoped consumer — so multiple shells (a classic OS view, a premium
   designed view, an AI shell) can coexist and be **swappable**. Needs a
   shell-manager: which presentation holds which scope, and clean session handoff.
4. **Intent-led AI shell** *(net-new, depends on 1–2).* State an outcome; a
   contained **agent capsule** (zero ambient authority) compiles it into capsule
   operations, previews each gate, requests approval, acts, and shows the receipts.
   The agent proposes; the capability layer disposes.
5. **Pluggable intelligence — local or cloud** *(design now, build with 4).* The
   model's location is a privacy/capability choice, not an architecture change:
   run a local LLM on your own hardware (data sovereignty — intent never leaves the
   box) or grant a scoped capability to a hosted model. Same safety envelope either
   way. "Bring your own intelligence."
6. **Living-object presentation (Morphic/Godot canvas)** *(net-new, presentation
   only).* Render capsules as live, manipulable objects on a GPU canvas (the modern
   Morphic). Presentation only — the Rust trusted core stays the authority; the
   canvas is the face, never the gate.

## The experience we're building toward

A shift from *operating* a computer to *intending* an outcome — and being able to
**see exactly what's about to happen before it does**.

Each thing we built becomes something the user can perceive:

- **Trust → material.** Signed = solid; content-addressed = glass; unsigned =
  translucent (our `trust_level` + `signature_fingerprint`, made visible).
- **Powers → visible ports** on each capsule (the `authority` surface).
- **A proposed action → a visible circuit of authority** drawn between capsules
  (the gate preview rendered as light) — the resources and actions shown *before*
  anything runs.
- **Approval → a deliberate ceremony**, heavier for irreversible "one-way doors."
  The friction on irreversible acts is sacred — the one thing we never smooth away
  (anti-automation-bias by design).
- **Granted capability → a token of light** flowing to the provider; **revocation →
  the light snuffed out**.
- **Audit → a persistent timeline**, attested events wearing a signed halo with the
  signer DID (our attestation work).
- **Scope → altitude.** System = the overworld of all capsules; SelfOnly = inside
  one. The shell-swap "clicker" pulls the camera back to choose a shell.

The honest magic: none of it is faked sparkle — **every glow is a real capability,
every halo a real signature, every snuffed thread a real revocation.** It holds up
to a skeptic because it's bound to real mechanics.

### Illustrative user stories
- **Creator:** "release this album to paid holders, keep the masters sealed" — keys
  flow only to verified holders, royalties settle, masters stay sealed; the user
  *watched* it rather than trusting it.
- **Enterprise operator:** an agent proposes a multi-step migration; reversible steps
  run autonomously, one-way doors are held for explicit approval, and the **attested
  timeline is exported as the compliance record** — the audit *is* the interface.
- **Sovereign home user:** a **local** model organises personal data; an indicator
  confirms nothing left the machine.
- **Contained agent (dogfood):** a coding agent works across capsules like a glass
  engine; when it reaches beyond its grant, the gate simply **denies** — safety seen
  happening in real time.

## Business model + enterprise

- **Shells as tiers.** Classic (free) · premium designed · AI shell (subscription).
- **Self-enforcing access.** The AI shell is itself a capsule, so access is a
  capability you pay for — enforced by the **same rights/key-release machinery
  (DDRM)** we govern. Pay → license token → shell unlocks. The product polices
  itself with its own primitives.
- **Enterprise wedge — agent-safe computing.** Agents that act on real systems but
  are capability-contained, previewable, approval-gated, and attested. The audit
  trail we built is the compliance artifact. Defence in depth: even an imperfect
  model is contained by the system.
- **Possible integration surface:** a hosting model where tool/agent protocols (e.g.
  MCP-style servers) run as capsules and are capability-gated — "system-level safety"
  complementing model-level safety.

## Trust & security framing (why this is sound)

Two distinct trust boundaries, not to be conflated:

- **Build-time (authoring the trusted core):** privileged by nature — whoever builds
  the enforcing core can change it. This is universal to every OS (cf. Thompson,
  *Reflections on Trusting Trust*). The defence is a **minimal, auditable TCB +
  reproducible, signed builds + provenance on the runtime itself**, so a user runs a
  build they can verify.
- **Run-time (an agent operating inside):** hard-contained by capabilities — it
  cannot rewrite the core or self-grant. This is the agent "adhering inside the
  system," and it holds by design.

On open source: **open code is not open authority.** A malicious fork endangers only
those who run unverified builds (mitigated by provenance), and it **cannot mint
authorization** — key release stays behind the real key-provider + on-chain rights
it doesn't control. Code is public; keys, signed grants, and chain-rooted rights are
not. Open barriers are auditable barriers.

The real risks to invest against are the **signing/supply-chain trust root**,
**automation bias** in a too-smooth shell, and **TCB creep** — not forking.

## Real vs. vision (so this doesn't rot into hype)

- **Real today:** reflect, gate preview, fail-closed scope, attestation, one
  canonical path — i.e. everything the experience is *bound to*.
- **To build:** approval loop (1), dispatch (2, merge-gated), shell-manager (3), the
  AI shell + pluggable intelligence (4–5), the living-object canvas (6).
- Keep each milestone honest with the `KNOWN_GAPS` ratchet pattern: a gap is a
  build-visible `#[ignore]`d test, closed by deleting the `#[ignore]`.
