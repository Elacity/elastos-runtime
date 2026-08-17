# Consequence-aware effects

ElastOS uses one Runtime effect model for information, media, money, rights,
and physical systems. It does not create separate IT and OT authority stacks.
Policy and proof follow the operation's meaning and possible consequences, not
its transport.

[PRINCIPLES.md](../PRINCIPLES.md) owns the stable authority rules.
[ARCHITECTURE.md](ARCHITECTURE.md) owns layer responsibilities, and
[state.md](../state.md) records what the current branch proves. This document
defines the effect contract that connects those rules.

## One effect path

An operation follows the same path whether a human clicks a control or an agent
proposes it:

```text
human or agent intent
        |
typed Runtime resource and operation
        |
principal, session, capability, approval, and policy checks
        |
owning provider
        |
local adapter or Runtime-selected Carrier route
        |
destination Runtime and provider admission, when remote
        |
local service or device controller
```

A capsule cannot select a host path, peer, protocol, controller, backend, or
credential. HTTP, Carrier, CAN, a webhook, and an in-process call are adapters
below the contract. They may carry an operation, but they do not define its
authority or prove its result.

Operations still need different evidence and failure rules. Reading a cached
document is not the same as observing a temperature, transferring funds, or
starting a motor, even when each request enters Runtime through the same
interface shape.

## Contract terms

| Term | Meaning | What it does not prove |
| --- | --- | --- |
| Observation | A provider report about a named subject at a stated time | That the report is fresh, authentic, accurate, or suitable for a decision |
| Actuation | A request intended to change digital, economic, rights, or physical state | That the target accepted, executed, or completed the request |
| Consequence policy | Runtime-enforced authority, approval, timing, retry, and evidence requirements informed by the admitted provider contract | Authority granted by capsule metadata or the provider |
| Settlement | Runtime's durable knowledge of whether the requested effect happened | Mere transport delivery or a successful HTTP response |

These definitions describe meaning, not a wire format. They do not require one
universal envelope or provider implementation.

## Classification and authority

Capsule manifests declare affordance risk so Home, CLI, Inspector, and Runtime
can present and plan an operation. That declaration cannot be the final safety
authority because the caller controls its own manifest.

The admitted provider contract describes operation semantics and hazards.
Runtime policy sets and enforces the minimum classification and may strengthen
it. A capsule may ask for stricter approval or audit, but it cannot weaken that
policy. A mismatch, missing classification, or unknown provider policy fails
closed. The provider may refuse an authorized request; it cannot mint the
caller's authority.

The current `AffordanceRisk` values mix access shape and consequence domains:
`read`, `write`, `launch`, `payment`, `rights`, `actuator`, and `privileged`.
They remain useful compatibility metadata. They are not a complete physical
safety model and should not grow into an informal substitute for provider
operation contracts.

## Minimum operation contract

Each effectful provider operation defines the parts that apply to its domain:

| Concern | Required contract |
| --- | --- |
| Binding | Typed resource, operation, input, output, target, and provider ownership |
| Authority | Required capability action, principal and session context, approval policy, and audit level |
| Consequence | What state may change and which provider or local controller owns the final admission |
| Timing | Request expiry or deadline, plus the meaning of a late result |
| Retry | Whether the operation is idempotent, requires a durable effect ID, or must be reconciled before retry |
| Settlement | What counts as accepted, executed, independently observed, proven not to have acted, or unknown |
| Safe failure | Preconditions, refusal behavior, cleanup, and any local interlock that remains authoritative |

These facts may live in the typed interface, provider authority metadata, and
Runtime policy. ElastOS should add a common schema only after two independent
provider families need the same fields. Until then, the semantic requirements
are common and the provider contracts remain specific.

## Observations

An observation used for an authority or safety decision needs enough evidence
for Runtime or the consuming provider to judge it. The contract names:

- the source principal, device, or provider;
- the subject being observed;
- the schema and, where relevant, value and unit;
- observation time, receipt time, and freshness or expiry;
- integrity or signature evidence;
- sequence, nonce, or another replay boundary; and
- quality or calibration evidence when the provider makes that claim.

A webhook can transport an observation, but receipt of the webhook proves only
receipt. A sensor report without source, freshness, and replay checks is
untrusted input.

## Actuation and local safety

An actuation request names a stable effect ID when replay could repeat a
consequence. It also carries an expiry or deadline and enough target binding to
prevent substitution.

Runtime authorizes and routes the request. The destination Runtime authorizes
remote requests again. The owning provider validates operation semantics. For
a physical effect, the device controller performs the final local safety check
and may refuse a valid Runtime request.

Remote membership, a DID, ownership evidence, a capability issued by another
Runtime, or successful Carrier delivery cannot bypass that local decision.
Emergency stops and fail-safe controls must remain available without Home,
Carrier, an agent, or a remote Runtime.

## Settlement and retry

Effect results must say what is known:

| Evidence | Meaning |
| --- | --- |
| Accepted by transport | Bytes reached the named endpoint |
| Accepted by provider | The provider admitted the request for processing |
| Executed | The provider reports that it issued or performed the operation |
| Observed | Bounded feedback confirms the resulting state |
| Did not act | The provider supplies evidence that the effect did not occur |
| Unknown | Runtime cannot prove whether the effect occurred |

The stronger states do not follow automatically from the weaker ones. After
dispatch, a timeout or lost route is `unknown` unless the operation contract
can reconcile the effect ID or prove that it did not act. Presentation recovery
must never repeat an effect merely because a UI, model stream, or Carrier route
reconnected.

## Ownership and control

Ownership, rights, and control are separate claims. A DID, title, token,
license, wallet proof, or signed Profile may contribute evidence to a policy
decision. It does not become an actuation capability by itself. Runtime still
checks the active principal, session, scoped capability, operation policy, and
required approval. A physical controller still applies local safety policy.

## Real-time boundary

ElastOS Runtime is not a hard real-time controller or a safety PLC. It owns
identity, authorization, routing, durable effect state, reconciliation, and
audit. Device providers and local controllers own deterministic timing,
hardware interlocks, emergency behavior, and safe state transitions.

This boundary keeps device-specific code outside the trusted Runtime without
allowing a provider to mint authority.

## Layer responsibilities

| Layer | Owns | Must not own |
| --- | --- | --- |
| Home or Agent Host | Intent, review, approval presentation, and durable status | Grants, provider credentials, or effect settlement truth |
| Runtime | Principal and session checks, capabilities, approval state, routing, effect identity, audit, and durable settlement knowledge | Device protocol behavior or hard real-time control |
| Provider | Typed operation semantics, declared consequence requirements, validation, reconciliation, and result translation | Capability minting or UI-owned approval |
| Carrier | Endpoint-authenticated off-box transport selected by Runtime | Application authority or effect completion claims |
| Destination Runtime | Independent admission of a remote service operation | Trust inherited from the source Runtime |
| Local controller | Device limits, interlocks, deterministic execution, and physical feedback | Ambient capsule authority or remote policy bypass |

Human and agent parity means both use this path and the same provider
operation. It does not mean they receive equal grants. A model or harness may
propose an actuation; it cannot approve or authorize its own proposal.

## Current implementation boundary

The dated implementation facts and open proof belong in
[state.md](../state.md#consequence-aware-effect-truth). As of 2026-08-16, this
branch does not ship a general physical-effect provider or a universal effect
state machine. This contract does not turn its target semantics into product
claims.

Do not call the first physical provider ready until a stranger can verify all
of the following on the installed target:

- the typed resource and operation, including negative input tests;
- Runtime-enforced provider operation classification;
- denial for missing, expired, revoked, replayed, or insufficient authority;
- hidden controller, backend, peer, credential, and transport details;
- effect-ID deduplication and unknown-settlement recovery;
- a local interlock refusal that remote authority cannot bypass;
- truthful accepted, executed, observed, did-not-act, and unknown results; and
- source, built, and installed artifact parity with target-local evidence.

## Non-goals

This contract does not add an OT quadrant, expose raw field buses to ordinary
capsules, turn Runtime into a PLC, create a global device namespace, or treat
digital ownership as control authority.
