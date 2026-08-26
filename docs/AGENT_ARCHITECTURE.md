# Human and agent architecture

This document defines how human and AI actors fit the same ElastOS authority
model. It is an architecture contract, not a claim that the complete agent
product is implemented. Current implementation truth and known gaps remain in
[`state.md`](../state.md).

The governing rule is:

> Humans and agents use the same Runtime resources, capability checks, session
> boundaries, and audit path. They may use different proofs and interaction
> flows, but neither receives ambient authority.

This follows [Principle 7](../PRINCIPLES.md#7-humans-and-agents-share-one-authority-model),
[Principle 13](../PRINCIPLES.md#13-objects-capsules-and-spaces-must-stay-distinct),
and [Principle 18](../PRINCIPLES.md#18-executable-capsules-are-isolated-execution-environments).

## Decision summary

- A human and an agent are both represented by Runtime principals.
- In the current collaboration model, a human's portable product identity is a
  signed Profile document with a stable Profile DID, controlled from a
  principal-owned protected `ProfileAuthority` bundle.
- A Profile is signed principal-owned data. It is not normally a capsule.
- Executable agent software is a capsule, separate from the agent principal
  and profile.
- A capsule instance acts only in the context of its capsule identity,
  principal, session, granted capabilities, resources, and lifecycle.
- A model or harness may propose an effect. Runtime alone authorizes and routes
  that effect.
- Human approval and agent delegation must resolve to the same underlying
  capability-scoped provider operation.
- A Profile may be exported as a signed data package, but importing or viewing
  it does not grant authority.

## Terms and ownership

In this document, **Profile** means the canonical signed public identity
document. It does not mean an installation profile selected by `elastos setup`,
a Browser profile disk, an execution profile, or a container for private agent
configuration. A future agent identity document and private agent persona state
are target architecture and remain separate objects.

| Concept | Meaning | Owner | What it is not |
| --- | --- | --- | --- |
| Principal | Runtime authority subject representing a human, agent, device, capsule, or provider | Runtime | A display name, passkey, DID, or capsule package |
| ProfileAuthority | Protected principal-root bundle containing the Profile signing secret, current signed public Profile, and retained revisions | Runtime-owned principal state | A public document, App, or ambient signing service |
| Profile | Signed public identity document containing a stable Profile DID, display data, monotonic revision, previous-profile hash, and authorized delivery-device bindings | Principal through ProfileAuthority | The local principal, passkey, device DID, or executable code |
| Profile DID | Stable person/contact identity named by signed People, discovery, contacts, and conversations | Signed Profile document | A local authorization principal or transport endpoint |
| Proof binding | Evidence accepted by Runtime as control of a principal, such as a human passkey or an agent proof | Runtime identity boundary | The principal itself |
| Session | Short-lived active context binding a principal, proof, lifecycle, and permitted operations | Runtime | A durable identity or profile |
| Capsule artifact | Immutable signed software or packaged-data artifact | Publisher and Runtime admission policy | A running agent or human account |
| Capsule instance | One isolated activation of a capsule under a session and capability set | Runtime lifecycle | The principal that uses it |
| Capability grant | Narrow Runtime authority over a resource, action, and operation | Runtime | A UI state, route, profile field, or model suggestion |
| Mandate (target) | Higher-level delegated policy that may constrain duration, scope, rate, or budget before Runtime grants an operation | Consent or mandate service; enforced by Runtime capabilities | Identity or ambient access |
| Agent private state (target) | Persona, model policy, memory roots, and selected skills or Apps | Agent principal through typed Runtime resources | A public Profile, signing authority, or executable capsule |

The canonical definition of a principal is in the
[Glossary](GLOSSARY.md#principal). Wallet addresses, passkeys, and DIDs are
proof bindings or linked identities; they do not replace the Runtime
principal. The current collaboration contract further distinguishes the local
principal from the signed [Profile DID](GLOSSARY.md#profile-did) that other
people name.

## Human and agent parity

Parity means equal enforcement, not identical embodiment.

| Concern | Human | Agent |
| --- | --- | --- |
| Authority subject | Human principal | Agent principal |
| Rooted state | `Users/...` | `UsersAI/...` |
| Typical proof | Passkey-backed session | Agent key or another Runtime-accepted proof |
| Product identity | Signed Profile DID | Verified agent identity document; target, not currently shipped |
| Identity authority and document | Protected ProfileAuthority and signed public Profile | Protected authority and signed agent identity document; target, not currently shipped |
| Private preferences and memory | Principal-owned application state | Principal-owned persona, model policy, memory, and skill selection |
| Executable actor | Home or another app capsule | Agent Host or task capsule |
| Immediate authority | Interactive approval or existing grant | Existing grant or interactive approval |
| Standing authority | Explicit user-configured policy | Explicit mandate scoped to the agent |
| Effects | Typed Runtime resources and providers | The same typed Runtime resources and providers |
| Audit | Runtime audit | Runtime audit |

The parallel shape is:

```text
Human principal                         Agent principal
  +-- protected ProfileAuthority          +-- signed agent identity (target)
  +-- signed public Profile DID           +-- private persona/model state
  +-- passkey proof                       +-- agent proof binding
  +-- active session                      +-- active session
  +-- Home/app capsule instance           +-- Agent Host capsule instance
             |                                        |
             +--------------+  +----------------------+
                            v  v
                   Runtime capability check
                            |
                            v
                    provider operation
                            |
                            v
                      audited result
```

The pointer, keyboard, API, and agent-message paths may begin differently, but
they must converge before the effect occurs. If a person and an agent request
the same operation on the same resource, Runtime must evaluate both through
the same authority and provider boundary.

Automation is therefore not a privileged bypass. It is the same operation with
more explicit delegation, expiry, revocation, and accounting.

## Why a profile is not normally a capsule

A Profile is a signed public identity object, and its protected
ProfileAuthority changes independently of executable software. The current
Profile schema contains display data, revision history, and explicit delivery
endpoint and application-signer bindings. ProfileAuthority contains the signing
secret, current signed head, and retained signed revisions.

Private preferences, persona, model policy, memory roots, and selected skills or
Apps belong in separate principal-owned objects. They may influence how an agent
acts, but they are not public identity fields and must not expand
ProfileAuthority into a general private-state bundle. A later Profile schema may
add explicitly public presentation or credential fields through a reviewed
versioned contract.

Making that live object an executable capsule would collapse four boundaries:

1. **Identity and code.** Changing public display data or private preferences
   should not replace the executable agent package.
2. **Mutable and immutable state.** Profile updates should not require a new
   capsule artifact and signature.
3. **Principal and actor.** Authority belongs to the principal and session,
   not to whichever profile viewer happens to be open.
4. **Shared implementation and individual identity.** Many principals should
   be able to use the same admitted Home, People/Profile App, or Agent Host
   capsule while retaining separate state and authority.

The current product uses the `people` App capsule as the Profile and contacts
surface, backed by Runtime-owned ProfileAuthority. A future dedicated Profile
App may also be a capsule. In either case, the capsule only views or requests
edits to Profile objects through Runtime-mediated operations. It does not
become the Profile, hold the Profile signing secret, or inherit the principal's
authority from being displayed in Home.

A Profile snapshot may later be packaged as a signed data capsule when
portability or provenance requires it. That packaging is optional target
architecture, not a current implementation claim. The exported artifact must
not become a bearer credential, and installing it must not silently recreate
sessions, capabilities, or mandates.

## Agent composition

An ElastOS agent is a composition rather than one package:

```text
agent principal
  +-- signed agent identity document (target)
  +-- private persona and model policy
  +-- principal-owned memory and conversation state
  +-- proof binding
  +-- selected Agent Host capsule
  +-- active sessions
  +-- capability grants and mandates
```

The **Agent Host capsule** owns ordinary harness behavior such as conversation
state, context construction, model selection, planning, compaction, and
interpreting model tool requests. Different harness implementations may fill
this role without changing the Runtime authority model.

The Agent Host is not trusted to authorize its own effects. It starts without
ambient filesystem, network, wallet, provider-credential, or control
authority. Every external effect crosses a typed Runtime resource.

One admitted Agent Host artifact may serve many agent principals. Each running
instance must be bound independently to its agent principal, session, state,
capabilities, resources, and lifecycle. Updating the harness must not rewrite
the agent's public identity, private persona, or memory.

## Runtime and provider architecture

The minimal agent path on ElastOS is:

```text
Home
  | start, resume, cancel, or present approval
  v
Runtime
  | bind principal, proof, session, capsule, grants, and lifecycle
  v
Agent Host capsule
  | harness, context, conversation, and planning
  |
  +-- model request --> Runtime --> model provider --> configured backend
  |
  +-- proposed effect --> Runtime capability check --> owning provider
                                                        |
                                                        v
                                                   audited result
```

The boundaries are:

- **Home** presents sessions, activity, approvals, results, and receipts. A UI
  card is not an authority grant.
- **Runtime** verifies the principal and session, enforces capabilities, owns
  lifecycle and audit, and routes the operation.
- **Agent Host** owns agent-loop behavior but cannot mint authority.
- **Model provider** owns model-backend credentials, protocol adaptation,
  model availability, and inference semantics. The Agent Host should not
  receive upstream credentials or endpoints.
- **Effect providers** own operation semantics for resources such as content,
  messaging, Browser, wallet, chain, rights, or storage. They do not decide
  that an unverified caller is authorized.
- **Carrier** may transport an off-box operation selected by Runtime. It is not
  the agent API or authority system.

## Request flow

A conforming agent action follows this sequence:

1. Home asks Runtime to start or resume an agent session.
2. Runtime resolves and binds the human principal, agent principal, proof,
   Agent Host capsule identity, session, and launch authority.
3. Runtime launches the isolated Agent Host instance with only its initial
   scoped resources.
4. The Agent Host requests inference through the model provider.
5. The model may return text or propose a typed operation. A proposal carries
   no authority.
6. The Agent Host submits the proposed operation to Runtime as the active agent
   principal and session.
7. Runtime verifies the exact resource, action, operation, principal, session,
   capsule, grant, expiry, and revocation state.
8. If additional consent is required, Runtime records a pending request and
   Home or Inbox presents it. The UI does not mint the grant itself.
9. Runtime dispatches an approved operation to the owning provider and records
   the result in the same audit path used for a human action.
10. The result returns to the Agent Host and Home without exposing provider
    credentials or broader authority.

## Session and interaction contract

An agent task is durable Runtime-owned state, not a short-lived Runtime session
or the lifetime of one terminal, web page, model stream, or Agent Host process.
It has a stable task ID and may span renewed session IDs. Model requests,
proposed effects, approvals, effect settlement, and presentation events record
both the task and the session that authorized them.

The minimum lifecycle is:

```text
created -> running -> completed
                   -> failed
                   -> cancelled
                   -> interrupted -> resumable -> running
        -> waiting_for_approval -> running
                                -> cancelled
                                -> interrupted -> resumable
```

`interrupted` means execution stopped before the system could establish a
terminal result. `resumable` means Runtime has enough durable state to continue
without recreating already settled work. A UI disconnect does not change the
task state by itself.

The interaction contract requires:

- one durable identity for the task, session, model request, approval request,
  and external effect;
- visible current state, progress, selected provider and model, and last durable
  event;
- an explicit interrupt or cancel operation that reports its settlement;
- approval requests that remain pending across UI, Agent Host, or Runtime
  restart until approved, denied, expired, revoked, or cancelled;
- resume from the last durable event after UI, Agent Host, or Runtime restart;
- one terminal state for every model request and external effect;
- deduplication by effect ID so presentation recovery cannot repeat a settled
  operation; and
- an explicit unknown-settlement state when Runtime cannot prove whether a
  remote or provider effect completed.

The UI must not remain in `responding` after the model provider has emitted a
terminal event. It must also not hide an approval because the view changed or a
stream disconnected. Reconnecting presentation reads durable state; it does
not rerun inference or submit the proposed effect again.

The [model provider contract](MODEL_PROVIDER.md) defines inference selection,
stream events, cancellation, and terminal outcomes. Provider-specific stream
formats remain behind that boundary.

## Harness conformance

Agent Host implementations may differ in prompts, context policy, planning,
memory, compaction, subtask execution, and user experience. To fit ElastOS,
each harness must:

- run as an admitted capsule instance without ambient filesystem, network,
  wallet, provider-credential, or control authority;
- bind every model request and effect proposal to the active agent principal,
  Runtime session, and durable task;
- use typed Runtime resources instead of direct host tools or provider
  endpoints;
- treat model output and tool calls as untrusted proposals;
- preserve cancellation and durable correlation IDs across its internal loop;
- keep private persona, memory, and conversation state in principal-owned
  resources; and
- accept Runtime denial, expiry, revocation, and unknown settlement as
  authoritative outcomes.

These constraints allow a harness to be replaced without creating another
authority system or changing the agent's identity.

## Delegation and mandates

Interactive and automated authority should differ only in delegation scope:

- **Allow once** authorizes one exact operation.
- **Allow for this session** authorizes a bounded operation set until the
  session ends or the grant is revoked.
- **Standing mandate** delegates a bounded operation set to an agent principal
  with explicit scope, expiry, revocation, and optional rate or spend limits.

A mandate is policy above the Runtime capability primitive. Runtime remains the
enforcement point for every resulting effect. A mandate service must not hand
the agent a user's unrestricted credentials or create a second authority
system beside Runtime.

## State ownership

- Human state is rooted under the human principal's `Users/...` space.
- Agent state is rooted under the agent principal's `UsersAI/...` space.
- Public Profile documents and private memory, conversation, and task state are
  mutable objects, separate from immutable capsule artifacts.
- Model credentials and provider topology remain provider-owned state.
- Runtime owns sessions, grants, revocation, lifecycle, and authoritative
  audit records.
- A public profile projection exposes only fields the principal explicitly
  publishes; a route or visible page does not grant access to private state.

This document does not freeze the exact path layout below `Users/...` or
`UsersAI/...`. Those paths require their own versioned object contract before
applications may depend on them.

## Current implementation status

The repository already establishes several parts of this model:

- `Users` and `UsersAI` are canonical local roots.
- Runtime principals may represent humans, agents, devices, capsules, or
  providers.
- Human collaboration uses a protected ProfileAuthority and signed Profile DID
  distinct from the local principal, passkey, and device DID.
- The `people` App is the current Profile and contacts surface; Profile
  authority remains Runtime-owned rather than becoming a Profile capsule.
- Capability and audit policy already applies to typed model-provider
  resources.
- The old terminal `chat` and `agent` source capsules are retired. Product Chat
  is the `chat-room` App; no general Agent Host is currently in the install set.
- The architecture already separates mutable capsule state from immutable
  capsule artifacts.

The complete parity path is not yet proven. In particular, `state.md` records
that principal, proof binding, device, capsule, launch grant, and session are
not yet independently established end to end for the Component path. The
repository does not yet ship a general Agent Host with durable task/session
recovery, persistent approvals, and arbitrary governed tools. It also does not
yet prove a corresponding signed agent identity document, private persona
contract, or standing mandate product end to end.

No product-readiness claim should be inferred from this architecture document.
A conforming implementation must prove the exact installed Runtime, capsule,
provider, principal, grant, session, restart, and audit path that a user runs.

## Non-negotiable boundaries

- A human profile is not a capability.
- A signed agent identity or private persona is not executable authority.
- A capsule signature admits code; it does not authorize user-scoped effects.
- A model response or tool call is a proposal, not permission.
- A harness is not the security boundary.
- A visible approval card is not the grant authority.
- A provider implements an operation; Runtime authorizes the caller.
- A passkey, DID, or wallet address proves or links identity under its stated
  contract; none grants ambient administrator authority.
- Human and agent actions must not use hidden alternate provider or transport
  paths.
- ElastOS must have one enforcing Runtime authority path, not parallel
  capability systems whose decisions can disagree.

## Related documents

- [Principles](../PRINCIPLES.md)
- [Architecture](ARCHITECTURE.md)
- [Glossary](GLOSSARY.md)
- [Capsule model](CAPSULE_MODEL.md)
- [Home shell host contract](HOME_SHELL_HOST_CONTRACT.md)
- [People and conversations](PEOPLE_CONVERSATIONS.md)
- [Model provider contract](MODEL_PROVIDER.md)
- [Private network contract](PRIVATE_NETWORK.md)
- [Current state](../state.md)
