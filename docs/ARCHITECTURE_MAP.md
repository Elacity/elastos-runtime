# ElastOS — capability architecture map

A one-screen mental model of the whole runtime. Read it top-to-bottom: **power increases
as you go down, but isolation increases with it.** Only the two double-lined layers (the
capability plane + the trusted core) are trusted; everything else — including the providers
that physically hold your keys — is sandboxed and starts at zero authority.

```
        ┌──────────────────────────────────────────────────────┐
 IDENTITY│  passkey / DID  →  principal                         │
        │  humans AND AI agents pass the SAME gate              │
        └──────────────────────────────────────────────────────┘

 APPS · web ───────────────────────── start at ZERO authority ──
   ┌────────────┬────────────┬────────────┬────────────┐
   │ Home/System│ Library    │ Marketplace│ Wallet     │
   │            │ · Files    │            │ · Browser  │
   └────────────┴────────────┴────────────┴────────────┘

 SHELLS · rust → wasm sandbox
   ┌──────────────────┬──────────────────┬──────────────────┐
   │ Home/system/inbox│    Chat room     │ AI agents·UsersAI│
   └──────────────────┴──────────────────┴──────────────────┘
                            │
                            ▼   every call funnels through here
 ╔══════════════════════════════════════════════════════════════╗
 ║   CARRIER · CAPABILITY PLANE                                   ║
 ║   every call:  signed · scoped · revocable · audited           ║
 ╚══════════════════════════════════════════════════════════════╝
                            │
                            ▼
        ╔═══════════════════════════════════════════════╗
        ║   TRUSTED CORE · runtime   (Rust, native)      ║
        ║   validate · bind · route · audit              ║
        ║   ──── the ONLY fully-trusted code ────        ║
        ╚═══════════════════════════════════════════════╝
                            │
                            ▼
 PROVIDERS · rust → microVM (KVM) ──── dangerous powers, QUARANTINED
   ┌───────────────┬────────────┬──────────────┬────────────┐
   │ keys·decrypt· │   chain·   │  net·exit·   │  ai·ipfs·  │
   │     dKMS      │   wallet   │    tunnel    │    did     │
   └───────────────┴────────────┴──────────────┴────────────┘

 ─────────────── below: ADAPTERS, not the product truth ────────────
   ┌────────────┬────────────┬──────────────┬──────────────┐
   │ Base chain │ IPFS·Kubo  │ dKMS 2-of-3  │ Carrier p2p· │
   │            │            │    nodes     │  disk · gpu  │
   └────────────┴────────────┴──────────────┴──────────────┘
```

## Why this is powerful — and unique

- **The trust boundary is in the right place.** Only the core + capability plane are trusted;
  everything else, *including the key-holding providers*, is isolated. You trust a small piece
  of auditable code, not the company.
- **Zero ambient authority, one chokepoint.** Nothing skips the capability plane. An app or an
  agent can do *nothing* until the core grants a scoped, signed, revocable, audited capability.
- **The trusted core is deliberately tiny** — the only thing you must get right. Everything
  dangerous is pushed out into microVM-isolated providers, and a drift gate stops the core from
  silently swelling. (See `adr/0001` + the trusted-core freeze.)
- **Graded isolation, strongest where the risk is:** frame (apps) → WASM (shells) → microVM/KVM
  (providers that touch keys and chain).
- **Humans and AI agents pass the same gate** (`Users` and `UsersAI` in parallel). An agent is a
  capsule with named, audited, mid-action-revocable capabilities — not a process with your shell
  permissions behind a prompt. Almost no one else has this.
- **Use-without-holding:** a provider can *use* a secret and return only the result — the key
  never crosses back up. dDRM is the hardest case of this; it generalizes to AI-agent custody.
- **Local-first:** the chain, IPFS, even the dKMS quorum are *adapters below the line*, not the
  truth. The user's machine + the rooted object model is the source of truth.

**Why it's unique (not just good):** plenty of systems have *one* of these — a sandbox, or
capabilities, or local-first, or DRM. The rare thing is *all of them, coherent, under one
authority model*: the same rule (named, narrow, revocable, audited) runs from the smallest key
release at the bottom up through the providers, the core, the agents, and out to a content
economy and an agent-custody business — humans and AI before the same law. That coherence can't
be retrofitted onto a system built on ambient authority, which is every cloud you'd compete with.

> Companion docs: `PRINCIPLES.md` (the constitution), `convergence/PRODUCT_VISION.md` (what/why),
> `CAPABILITY_AUDIT.md` + `SECURITY_AUDIT.md` (verification that the above holds in code).
