# Confidential Computing (TEE / Hardware Enclaves) — Opportunity & Architecture Scaffold

> **Status: forward design / research. NOTHING here is implemented yet.** As of 2026-06-16 the
> runtime launches plain `crosvm` microVMs (KVM isolation, no memory encryption, no attestation).
> The only TEE references in-tree are this doc, roadmap notes, and a comment in
> `capsules/ddrm-envelope/src/access.rs` about a third party's enclave. This document states the
> opportunity, the wedges, and a buildable plan so the work can be picked up (in Cursor) without
> re-deriving any of it.
>
> Companion docs: [THREAT_MODEL.md](THREAT_MODEL.md) (what we do / don't defend today),
> [SECURITY_AUDIT.md](SECURITY_AUDIT.md), [ARCHITECTURE.md](ARCHITECTURE.md),
> [PRINCIPLES.md](../PRINCIPLES.md). This doc follows §11 (fail closed, then explain) and §12
> (docs/code/tests/ops must agree) — if code later contradicts a claim here, that is a bug in one
> of them; file it.
>
> **Scope discipline:** dDRM bias removed on purpose. TEE is assessed as a runtime-wide primitive.
> dDRM is *one* consumer, not the headline.

---

## 0. TL;DR (the one thing to take away)

A microVM protects the host **from the capsule**. A TEE protects the capsule's data **from the
host/operator** — and emits a **remote attestation**: a cryptographic proof a third party can verify
that *this exact code ran in a sealed enclave and the operator could not read the data inside it.*

That single property — **remote attestation collapses trust into a proof** — is what turns ElastOS
from "a sovereign OS you run yourself" into "a marketplace of providers and nodes you can use
**without trusting whoever operates them**." It is the missing trust leg that lets the
provider / node / custody economy scale to *untrusted* operators, and it directly closes the
single most valuable open item in [THREAT_MODEL.md](THREAT_MODEL.md) §6.

**Recommended first wedge:** attested dKMS nodes. It is already on the roadmap path (node
attestation / slashing), it closes the metadata-leak limitation, and it is the smallest credible
proof of "attested — you don't have to trust us."

---

## 1. What a TEE adds that a microVM cannot (the whole argument in one table)

Each row is a distinct adversary. The point of the table is that the layers are **orthogonal** —
TEE is not a replacement for anything we shipped; it fills the two cells microVM leaves empty.

| Threat / adversary | microVM (crosvm, today) | **TEE adds** | quorum 2-of-3 (today) | PQ-hybrid crypto (today) |
|---|:--:|:--:|:--:|:--:|
| Capsule escapes to host / sibling capsule | ✅ KVM isolation | — | — | — |
| **Host/operator reads guest memory** (plaintext, CEK, key shares, prompts) | ❌ host owns RAM | ✅ memory encrypted by the CPU, host-opaque | — | — |
| **A remote party can *verify* the code that ran** | ❌ unverifiable | ✅ remote attestation (signed measurement) | — | — |
| A single party reconstructs the CEK | — | — | ✅ needs ≥2 shares | — |
| Data/key at rest & in transit; harvest-now-decrypt-later | — | — | — | ✅ x25519+ML-KEM, ML-DSA, AES-GCM |

**Read it as:** TEE = *can't see it*, quorum = *can't single-handedly reconstruct it*, crypto =
*can't break it later*. Three adversaries, three layers, defense in depth. A TEE break (see §8)
degrades to "still need ≥2 colluding shareholders" — the quorum is the backstop, and vice versa.

### 1.1 The honest definition of "attested"
"Attested" does **not** mean "trust no one." It means: *trust the silicon vendor's root of trust
(AMD/Intel) plus the measured enclave code — and nothing else, in particular not the operator, the
OS, the hypervisor, or the cloud.* The TCB shrinks from "the whole operator stack" to "the CPU +
the code you measured." That is a profound reduction, but it is a **shift** of trust, not its
elimination. Every claim we make externally must be stated exactly this way (§8).

---

## 2. Where it applies across the runtime (dDRM bias removed)

Ranked by strategic leverage. Each is a place where today a user must *trust an operator* and TEE
replaces that trust with a *proof*.

1. **Attested dKMS nodes — the keystone.** Today a node operator serving a recover sees the
   `(wallet, content_id, time)` access pattern and handles a key share in cleartext
   ([THREAT_MODEL.md](THREAT_MODEL.md) §3, §6.1). Inside a TEE the operator *runs* the node but
   *cannot see inside it*, and proves so by attestation. This closes the metadata leak, hardens the
   honest-operator assumption, and is the precondition for **permissionless attested nodes** (anyone
   can run a node; clients verify the attestation before trusting it with a share). Converges
   exactly with the node-attestation / slashing roadmap.

2. **Confidential compute / AI inference — the highest ceiling.** Protecting a *key* never protected
   *data-in-use* from the compute operator or the model vendor. A TEE does: run a model or an
   analytics job inside an attested enclave; inputs, weights, and outputs are host-opaque, and you
   can *prove* it. Unlocks regulated AI (health, finance), model-weight protection for model owners,
   and multi-party **data clean rooms** — the verticals key-custody alone cannot serve.

3. **Attested Agent Key Vault — the enterprise custody closer.** "Sign-without-holding" plus
   "*not even we can see your agent's keys — here is the attestation*." Kills the only real objection
   to the sovereignty pitch ("aren't you just the new landlord?"): the proof says there is no
   landlord.

4. **Attested provider marketplace over Carrier** (Rong Chen's runtime-users-as-providers vision).
   Any participant runs a provider/node; consumers verify its attestation *before* releasing
   plaintext, a share, or a capability to it. Carrier is discovery + transport for the attestation
   exchange. This is the platform play, not a feature.

5. **Sovereign / regulated workloads.** "Provably ran sealed; the operator could not exfiltrate"
   is a data-residency and compliance primitive (defense, finance, gov, EU data-sovereignty) that
   nothing in the current stack delivers.

---

## 3. Architecture — how it slots into what exists

Three integration surfaces, all of which already exist in the tree. The design principle: **TEE is
an attribute of where a capsule runs and a precondition on what may be released to it** — it is *not*
a new trusted-core subsystem (keep §5 small-core; the verifier is a library, the policy is a
capability constraint).

```
                        ┌────────────────────────────────────────────────┐
                        │  capability plane  (elastos-runtime/capability) │
                        │  TokenConstraints + require_attestation(measmt)  │  ← (C) policy
                        └───────────────┬────────────────────────────────┘
                                        │  "release only to an enclave measuring X"
        Carrier (discovery/transport)   │
   peer ── attestation report ────────► │ verify_before_release
        ◄─ challenge/nonce ───────────  │  (B) attestation verifier (library)
                                        ▼
   ┌──────────────────────────────────────────────────────────────────────┐
   │ elastos-crosvm : Vm::start → config.to_crosvm_args()                   │  ← (A) launch
   │   + ConfidentialVmMode::{SevSnp|Tdx|None}   (fail-closed if required   │
   │   + capture attestation report at boot       but unavailable)          │
   └──────────────────────────────────────────────────────────────────────┘
```

### (A) Confidential-VM launch — `elastos/crates/elastos-crosvm`
The launch path is `Vm::start(&mut self, crosvm_bin)` in `src/vm.rs`, which spawns
`crosvm run --socket <sock> <args>` where `<args>` come from `config.to_crosvm_args()` in
`src/config.rs`. crosvm already supports confidential guests (AMD SEV-SNP; Intel TDX). The work:

- Add `ConfidentialVmMode { None, SevSnp, Tdx }` to `VmConfig` (default `None` — behavior-preserving).
- `to_crosvm_args()` emits the confidential-guest flags when the mode is set.
- `Vm::start` already checks `/dev/kvm`; add a **capability/hardware probe** (SEV-SNP / TDX present
  and enabled). If a capsule manifest *requires* confidential mode and the host can't provide it,
  **fail closed** (`PRINCIPLES` §11) — never silently downgrade to a plain microVM.
- After boot, **capture the attestation report** for the launched guest and surface it to the
  supervisor so it can be advertised (D) and gated on (C).

### (B) Attestation verifier — new small library crate (e.g. `elastos-attestation`)
Pure verification logic, no I/O, easy to test and audit (mirrors how `ddrm-envelope` is a pure
crypto library, not a service). Responsibilities:

- Parse a vendor attestation report (SEV-SNP attestation report / TDX quote).
- Verify the signature chain to the **pinned vendor root** (AMD ARK/ASK, Intel PCS roots) — roots
  pinned in config, exactly like the dKMS node's pinned operator key is pinned today.
- Check **freshness** (the report binds a caller-supplied nonce — bind it to our channel challenge,
  reusing the dKMS session-token challenge pattern in `dkms-authority`).
- Compare the **measurement** against an expected/allow-listed value → return a typed
  `VerifiedEnclave { measurement, vendor, tcb_version, .. }` or a fail-closed error.
- **Crypto-agility tag** on the verdict (as the audit log carries `alg`) so a vendor/format rotation
  is a zero-drama change.

### (C) Attestation-gated capability — `elastos/crates/elastos-runtime/src/capability/token.rs`
`TokenConstraints` today carries `{ epoch, delegatable, max_classification, max_uses }`. Add an
**optional** attestation constraint so a capability can be scoped to a verified enclave:

- `require_attestation: Option<ExpectedMeasurement>` (None = today's behavior exactly).
- Enforced **fail-closed at the release/use site** alongside the existing checks: a token that
  requires attestation is unusable until a matching `VerifiedEnclave` (from B) is presented for the
  target. This is the capability-plane expression of "release the CEK / share / key **only** to an
  enclave running measurement X."
- Stays within §3 (no ambient authority) and §16 (narrow, revocable): attestation is one more
  explicit, checked precondition on an already-scoped token, not a new authority.

### (D) Carrier attestation exchange — discovery + verify-before-trust
A provider/node advertises its attestation (its measurement + a fresh report) over Carrier; a
consumer runs (B) **before** sending plaintext, a share, or an attestation-gated capability (C).
This is the wire-level handshake that makes (2) and (4) real. It is additive to the existing
channel (hybrid x25519+ML-KEM AEAD, ML-DSA-signed) — the attestation rides as a signed,
challenge-bound preface, reusing the channel's domain-separation discipline.

### 3.1 Software attestation already exists — TEE is the hardware upgrade
The dKMS node *already* does a **software** attestation: a pinned operator ML-DSA verifying key
(`DKMS_AUTHORITY_*` env, fail-closed when absent) plus a node-signed session token binding the
client challenge (`capsules/dkms-authority/src/main.rs`). That proves *"a node holding the pinned
key answered."* It does **not** prove *"the operator can't see inside the node."* TEE hardware
attestation is the strict upgrade: same challenge-binding discipline, but the proof now covers the
**code and the memory confidentiality**, not just key possession. Layer them: pinned-key identity +
hardware attestation = "the right node, and a sealed one."

---

## 4. Wedges, ranked (ROI × feasibility)

| # | Wedge | What it closes / unlocks | Lift | Why now |
|---|-------|--------------------------|------|---------|
| 1 | **Attested dKMS nodes** | THREAT_MODEL §6.1 metadata leak; honest-operator assumption; permissionless nodes | M | On the existing attestation/slashing path; smallest credible "don't-trust-us" proof |
| 2 | **Confidential inference enclave** | Data-in-use from operator/model vendor; health/finance AI; clean rooms; model-weight protection | L | Highest revenue ceiling; the verticals key-custody can't serve |
| 3 | **Attested Agent Key Vault** | The "new landlord" objection to sovereignty/custody | M | Converges with the sign-without-holding agent-vault wedge already scoped |
| 4 | **Attested provider marketplace (Carrier)** | Trust at marketplace scale to untrusted operators | L | Platform play; depends on 1 + (D) being proven first |
| 5 | **Sovereign/regulated workload mode** | Data-residency / compliance attestation | M | Sales-led; package 1–3 for a named regulated buyer |

Sequencing: **1 → (D) → 3 → 2 → 4/5.** Each step reuses the prior step's verifier and handshake.

---

## 5. Build plan (phased — each phase independently verifiable, gated by `just verify`)

Per [CLAUDE.md](../CLAUDE.md): smallest independently-verifiable steps, each with a one-sentence
check, plan approved before code, new gates start non-blocking.

- **Phase 0 — spike (no product code).** Stand up a SEV-SNP *or* TDX confidential VM on capable
  hardware (cloud confidential VM is fine), get crosvm to launch a guest in confidential mode, and
  pull one real attestation report end to end. *Check: a genuine report is captured and its
  signature chains to the vendor root.* De-risks every later phase.
- **Phase 1 — verifier library (`elastos-attestation`).** Implement (B) against the Phase-0 report
  as a fixture. Pure, fully unit-tested. *Check: `just test-crate elastos-attestation` — valid
  report verifies; tampered report / wrong measurement / stale nonce / wrong root all fail closed.*
- **Phase 2 — confidential launch in `elastos-crosvm`.** Implement (A): `ConfidentialVmMode`,
  hardware probe, fail-closed-if-required-but-absent, report capture. Default `None` keeps every
  existing path byte-identical. *Check: existing crosvm tests still green; a manifest requiring
  confidential mode on a non-capable host fails closed with a clear error.*
- **Phase 3 — attestation-gated capability.** Implement (C) in `capability/token.rs`;
  `require_attestation: None` is a no-op. Add to the `capability_conformance` harness a GAP that is
  **CLOSED only when** an attestation-required token refuses release to an unattested/mismatched
  target. *Check: `just test` + the new conformance case; `just alignment-check` green.*
- **Phase 4 — Carrier handshake (D).** Advertise + verify-before-release over Carrier, additive to
  the existing sealed channel. *Check: an integration test where a consumer refuses to send a share
  to a peer whose attestation doesn't verify.*
- **Phase 5 — wedge #1 wiring.** Make a dKMS node launch confidential (Phase 2), advertise its
  attestation (Phase 4), and have the client gate the share release on it (Phase 3). *Check: a node
  that cannot present a valid attestation gets no share; the THREAT_MODEL §6.1 row can be downgraded
  with code to back it.*

Update [THREAT_MODEL.md](THREAT_MODEL.md), [ROADMAP.md](../ROADMAP.md), and this doc **in the same
change** as the code that earns each claim (§12). Do not move a THREAT_MODEL "do NOT defend" item
until the test that defends it is green.

---

## 6. Interface scaffold (illustrative — for Cursor to refine, not final)

> Sketches to anchor the shape and the boundaries. Names/signatures will move; the *contracts*
> (pure verifier, fail-closed, optional-and-default-off, crypto-agile) should not.

```rust
// elastos-crosvm/src/config.rs  — (A)
pub enum ConfidentialVmMode { None, SevSnp, Tdx }      // default: None (behavior-preserving)

pub struct VmConfig {
    // ...existing fields...
    pub confidential: ConfidentialVmMode,
    /// If true and the host cannot provide `confidential`, Vm::start fails closed (no downgrade).
    pub require_confidential: bool,
}

// elastos-attestation/src/lib.rs  — (B)  pure, no I/O
pub struct ExpectedMeasurement { /* allow-listed launch measurement(s) + min TCB */ }
pub struct VerifiedEnclave { pub measurement: Measurement, pub vendor: Vendor, pub tcb: TcbVersion }
pub enum AttestationError { BadVendorChain, MeasurementMismatch, StaleNonce, UnsupportedFormat, /* .. */ }

pub trait AttestationVerifier {
    /// `report` from the peer; `nonce` is OUR channel challenge (freshness); roots are pinned.
    fn verify(&self, report: &[u8], nonce: &[u8], expected: &ExpectedMeasurement)
        -> Result<VerifiedEnclave, AttestationError>;   // fail closed on every doubt
}

// elastos-runtime/src/capability/token.rs  — (C)
pub struct TokenConstraints {
    pub(crate) epoch: u64,
    pub(crate) delegatable: bool,
    pub(crate) max_classification: Option<u8>,
    pub(crate) max_uses: Option<u32>,
    pub(crate) require_attestation: Option<ExpectedMeasurement>, // None == today's behavior
}
// Enforced at the release/use site: Some(expected) ⇒ a matching VerifiedEnclave MUST be presented,
// else the use fails closed — exactly like epoch/max_uses are checked today.
```

---

## 7. Why it complements ElastOS (not a detour)

- **Already microVM/crosvm-shaped.** "Providers become attested enclaves" is an *increment* on
  `elastos-crosvm`, not a re-architecture. Stacks not built on a sandboxed-provider model would have
  to rebuild; ElastOS is already in the right posture (`docs/ARCHITECTURE.md`, the "dangerous
  machinery lives in isolated providers" design).
- **It closes open items, it doesn't open a new front.** Wedge #1 directly retires
  [THREAT_MODEL.md](THREAT_MODEL.md) §6.1 (the "single most valuable thing to scope next" per that
  doc's own summary) and hardens §2's honest-operator assumption.
- **It composes with shipped crypto.** Quorum + PQ-hybrid stay exactly as they are; TEE adds the
  operator/host dimension and verifiability (the §1 table). A TEE compromise degrades to the quorum,
  not to plaintext.
- **It's the same trust discipline, upgraded.** Pinned-key software attestation already exists
  (§3.1); hardware attestation is the strict superset, reusing the channel's challenge-binding and
  the audit log's crypto-agility tag.

---

## 8. Honest caveats (no hype — required reading before any external claim)

- **"Attested" = trust the CPU vendor + the measured code, NOT "trust no one"** (§1.1). The hardware
  vendor's root and firmware are in the TCB.
- **TEEs have a real attack history.** Intel SGX and AMD SEV/SEV-SNP have had architectural and
  side-channel breaks over the years (memory-bus, ciphertext, single-stepping, rollback classes).
  Treat attestation as **defense in depth**, never the sole guarantee — which is *precisely* why it
  must compose with the quorum (a TEE break still leaves "need ≥2 colluding shareholders"), not
  replace it.
- **Hardware dependency.** Needs SEV-SNP (AMD EPYC) or TDX (Intel Xeon) — available as cloud
  confidential VMs and on newer on-prem silicon. This is an ops/deployment dependency, not just code;
  the runtime must run unchanged on non-confidential hosts (mode `None`), failing closed only when a
  workload *requires* confidentiality.
- **Lift is real but bounded.** Verifier (M), launch wiring (M), capability + Carrier (M each),
  per-vendor report formats and TCB/version management are the fiddly parts. Phase 0 de-risks before
  any product commitment.
- **Measurement & supply chain.** "Measurement X" is only meaningful if the enclave image is
  reproducibly built and the allow-list is governed. Attestation moves trust to *the thing you
  measured* — so the build pipeline becomes security-critical.

---

## 9. Enterprise & revenue angle (why this opens lucrative doors)

- **A funded category with regulatory tailwind.** Confidential computing is sold by every major
  cloud (Azure Confidential VMs, AWS Nitro Enclaves, GCP Confidential VMs) and a field of venture-
  backed independents (Anjuna, Fortanix, Edgeless/MarbleRun, Oasis, Phala). The buyers — finance,
  health, gov — have budget *and* compliance mandates that make "provably sealed" a requirement,
  not a nice-to-have.
- **"Attested — you don't have to trust us" is the universal closer.** It answers the #1 objection
  to any custody/sovereignty sale ("why not self-host or just trust the cloud?") with a proof, not a
  promise. It applies to every wedge in §2.
- **It's the prerequisite for the 7-figure verticals.** Confidential inference (wedge #2) and
  attested marketplaces (#4) are the deals key-custody alone cannot reach: model owners who won't
  ship weights, hospitals who can't expose PHI, banks who need data-in-use guarantees, clean rooms
  between mutually-distrusting parties.

---

## 10. Open decisions (for the team / external auditor)

1. **SEV-SNP first or TDX first?** Pick by the hardware you can get for Phase 0 (cloud confidential
   VM is the fastest path); the verifier (B) should be vendor-pluggable regardless.
2. **Measurement governance** — who owns the allow-list of acceptable enclave measurements, and how
   is it rotated/revoked? (Ties to the capability epoch model.)
3. **Where does the verifier run** for wedge #1 — client-side before share release, in a quorum
   peer, or both? (Recommendation: client-side first; mutual later.)
4. **Reproducible enclave builds** — what makes "measurement X" auditable end to end?
5. **External anchoring overlap** — the audit-log anchoring roadmap (THREAT_MODEL §4) and attestation
   freshness both want an external witness; design them once.

---

*Authored as a research/design scaffold for handoff. No runtime code changed. Pick up at Phase 0.*
