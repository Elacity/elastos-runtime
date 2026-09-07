# Model provider contract

Agent Hosts and Apps request inference through a typed Runtime resource. They do
not receive hosted-model credentials, provider endpoints, local model process
access, or authority to choose a hidden alternate backend.

This document defines the target contract needed by a durable Agent Host.
Current implementation truth stays in [`state.md`](../state.md).

## Authority boundary

```text
Agent Host or App
  -> Runtime capability check
  -> model provider operation
  -> configured local or hosted backend
  -> typed stream events and terminal result
```

Runtime authorizes the caller, resource, action, model policy, and session.
The model provider owns backend credentials, endpoint selection, protocol
adaptation, request serialization, streaming, rate-limit translation, and
provider-specific error handling. The model backend supplies output; it does
not gain Runtime authority.

A tool call or effect proposal returned by a model is untrusted input. The
Agent Host must submit it as a new typed Runtime operation under the active
agent principal and session.

## Deployment and placement

ElastOS has one typed model-provider contract: `offers_list`, `runs_create`,
`runs_get`, `runs_events`, and `runs_cancel`. A Runtime can use a configured
offer backed by a local engine or a hosted API. Backend placement and consumer
placement are independent:

| Backend | Same-Runtime use | Granted cross-Runtime use |
| --- | --- | --- |
| Local engine | Operator-configured local inference | Share the selected local capability after remote acceptance |
| Hosted API | Operator-configured hosted inference | Share after remote acceptance and upstream terms, privacy, and cost review |

Both consumption paths use the existing model operations and service-offer
contract. Cross-Runtime model use remains planned work. Carrier currently
excludes `model` from its provider target allowlist. `RuntimeCreateBinding` and
`RuntimeAccessBinding` describe local-channel authority and carry no
authenticated remote issuer.

An operator explicitly selects a configured capability for publication under
Runtime policy. The owning Runtime publishes it as an
`elastos.service.offer/v1` service and owns grants, quotas, selection, audit,
and routing. Before reusing provider invocation for remote model work, the
destination Runtime must verify the signed offer and grant, then map the
authenticated source Runtime and consumer principal, capsule, and run into
destination-owned authority. Adding an allowlist entry alone cannot establish
this binding. Carrier authenticates and transports the route that Runtime
selects; the destination grants model authority. Publication advertises the
selected capability. Destination-owned grants authorize its use. The host Home,
workspace, and other runs retain their separate access rules. Hosted credentials
and local model artifacts stay inside their owning boundaries.

The owning Runtime DID signs the service offer, which names the admitted
provider capability and contains only bounded capability and policy facts. The
provider identity remains an internal execution binding. Backend URLs,
credentials, process details, and topology stay inside the model provider. A
model artifact is separate immutable content. Its canonical package identity
is the CID of the complete manifest-and-payload closure. Engine, component, and
payload hashes are verification facts rather than package identities. Runtime
prepares and admits the package through the planned content provider path in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

A hosted web API is a provider-internal HTTPS interoperability edge on the
Runtime that owns the credential. Mac to local model provider to hosted API is
local configured use. Mac Runtime to Carrier to a Jetson or seed Runtime, then
to that Runtime's model provider and backend, is service use. Ordinary capsules
see only typed `elastos://model/*` resources. Publishing a paid hosted offer
requires operator-owned quota, accounting, and data-policy facts.

The OpenAI Responses API is one provider-internal hosted model adapter. It
creates model responses behind the same typed model-provider contract. It does
not provide local Codex agent execution.

Codex integration is a separate, later agent-execution adapter. It controls
local Codex agent threads behind typed agent operations. Runtime admits each
operation through explicit filesystem, network, tool, and approval grants.
Codex can use a model internally, but it is not a model offer and does not
appear in `offers_list`.

## Selection policy

A request uses one explicit selection mode:

| Mode | Meaning | Required record |
| --- | --- | --- |
| Pinned | Use one provider and model identifier | Requested and resolved provider/model |
| Provider auto | Let one named provider choose a model according to its documented policy | Provider, requested auto selector, and resolved model when reported |
| Runtime policy | Let Runtime select from an explicit allowlist under a named policy | Policy revision and selected provider/model |

The selection mode is part of the request and session facts. `auto` is not a
model identity and must not be displayed later as if it were the model that ran.
If the upstream provider reports the resolved model, the provider records it.
If it does not, the result says that the resolved model is unknown.

Fallback is opt-in. A policy must name the allowed providers or models, trigger
conditions, ordering, data-handling constraints, and cost or rate limits. A
timeout, refusal, context error, or malformed response must not silently switch
providers. The result records every attempted backend without exposing secret
configuration.

Changing provider or model changes inference behavior, not the agent principal,
Agent Host artifact, conversation identity, memory ownership, capabilities, or
mandates.

## Request identity and session facts

Runtime assigns or validates a durable request ID unique within the agent task.
The provider binds these facts before dispatch:

- agent and Runtime session identifiers;
- caller capsule instance;
- requested selection mode and policy revision;
- resolved provider and model when known;
- sampling and token limits;
- context or attachment references permitted for the call;
- creation, dispatch, first-event, and terminal timestamps;
- cancellation state;
- usage and cost facts reported by the backend; and
- one terminal outcome.

Secrets, raw provider credentials, and private endpoint details do not belong in
the session journal or App-visible result.

## Stream lifecycle

A model request follows one observable lifecycle:

```text
created -> dispatched -> streaming -> completed
                       |           -> failed
                       |           -> cancelled
                       |           -> reconciling -> completed
                       |                         -> failed
                       |                         -> cancelled
                       |                         -> settlement_unknown
                       -> failed
```

Every request reaches one terminal state. A provider must not leave the Agent
Host or UI in `responding` after the backend stream has completed, failed, or
closed. A terminal event contains the request ID, outcome, final usage when
known, and enough error classification for the caller to decide whether a new
request is safe.

Stream events use monotonically increasing sequence numbers. Reconnection asks
for events after the last durable sequence. The provider may replay already
emitted text events with their original sequence numbers, but the consumer must
deduplicate them. Reconnection cannot dispatch the model request again.

## Cancellation, interruption, and resume

Cancellation is a request to stop backend work and settle the request as
`cancelled`. The provider records the request before closing the backend stream.
If the backend cannot confirm cancellation, the request enters reconciliation
rather than being reported as safely cancelled.

The managed local engine and hosted Chat Completions and Responses adapters
settle cancellation as `settlement_unknown` once dispatch may have happened.
Closing an HTTP stream does not confirm that the backend stopped. Their terminal
result and events remain durable across replay and restart without redispatch.
Installed cancellation and backend-stop proof remain open.

Interrupting the presentation layer does not cancel provider work by itself.
The Agent Host can reconnect to the same request ID and recover durable events.
After an Agent Host or Runtime restart, the session journal must distinguish:

- a completed request whose terminal event can be replayed;
- a request still running and eligible for reattachment;
- a confirmed cancelled or failed request; and
- a request with unknown backend settlement that requires reconciliation.

Resuming a conversation creates a new inference request unless it is only
reattaching to an existing request. The UI must show which case occurred.

## Errors

Provider results use stable error classes while retaining a redacted backend
code for diagnosis:

- selection unavailable;
- credentials unavailable;
- authentication rejected;
- rate limited;
- context or input rejected;
- backend timeout;
- transport interrupted;
- backend failed;
- response malformed;
- cancelled; and
- settlement unknown.

Errors identify the failed request and whether any output was emitted. They do
not expose API keys, authorization headers, private URLs, or raw backend logs to
ordinary capsules.

## Configuration examples

Hosted gateways such as OpenRouter and local engines such as llama.cpp or
Ollama are provider implementations, not architecture. Their model identifiers
and availability can change independently of ElastOS.

Operators select a model or routing policy from the provider's catalog.
That configuration stays behind the provider boundary. Product UI should
show the requested selector, resolved model when known, and whether explicit
fallback was enabled. Canonical architecture documents do not freeze a
commercial model name or claim that a catalog entry will remain available.

Hosted configuration records privacy policy, cost and rate limits, requested
selector, resolved model when the backend reports it, and explicit fallback
policy. Credentials start in the current owner-only provider config. Secret
indirection can use an existing secure service when one is available; it does
not require a new secret store.

## Local content selection and retention

The current closeout includes the complete path from trusted model discovery
to a real local reply for one verified Qwen package. Runtime source now verifies
the bounded signed catalog metadata described in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md#implemented-catalog-metadata-profile).
Its model rows remain unprepared. Transfer, package admission, offer binding and
the following UI flow remain pending implementation.
Marketplace adds Models browse/details/Use within the existing app; System
manages the same model records and local retention. Assistant and Home Agent
keep their existing pickers and typed run lifecycle. These are projections of
one Runtime catalog, admission inventory and offer binding, not separate model
stores. A model remains identifiable by complete-closure CID even when its
bytes are not local.

Selecting a model may ask Runtime to prepare it under current authority. The
person sees availability, Preparing/progress, Ready or actionable failure,
offline and incompatibility states. Preparation can be cancelled or retried.
Ordinary on-demand use may cache bytes. Keep on this device requests explicit
retention; releasing Keep makes them evictable once active references and run
settlement permit removal. The same inventory distinguishes cache, Keep,
admission and readiness, while catalog identity stays visible. A local pin alone
proves neither trust nor engine readiness. There is no user GGUF download,
file-picker or private
path configuration step in this product flow.

Runtime validates signed publisher/catalog and package facts, resource policy,
engine compatibility and content integrity before deriving a private verified
artifact descriptor for the model provider. Startup currently loads static
operator offers; the new admitted-content binding must reuse the existing
ProviderRegistry and run journal and preserve other configured capabilities.
An idle-safe provider refresh may expose the newly admitted local offer; a busy
or unknown-settlement run prevents destructive reconfiguration and removal.
Content preparation status is distinct from a dispatched model run.

Existing `offers_list`, `runs_create`, `runs_get`, `runs_events` and `runs_cancel`
remain the inference contract. Runtime binds the selected package and offer to
the exact admitted descriptor; the provider revalidates it before inference.
Preparation failure never changes the selected model to an available substitute.
Drafts and existing runs survive selection, progress, cancel, retry and restart.
The ordinary explicit Send/run action dispatches once only when that selected
model and provider are ready.

The bounded transfer, manifest/inventory limits, proposed preparation/retention
intents, commit order and tests are in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md#source-prerequisites-and-bounded-implementation-plan).
Fresh-install acceptance must prove the compatible engine and its verified
libraries are available, then select the real signed catalog entry with no
pre-existing GGUF or private offer setup and obtain a real Qwen reply. Exact
package size, publisher trust, license/provenance and availability deployment
are prerequisites. Small signed fixtures prove rejection and lifecycle behavior,
not a production catalog or real inference. Code, tests, manifests and docs go
to Git review; model bytes and private publisher keys stay outside Git.

## Staged delivery path

Local-engine and hosted-API acceptance are separate tracks. Each backend must
pass installed lifecycle tests before it can be shared:

1. Evaluate Qwen3.5-9B Q4_K_M as the stable Mac baseline and PrismML Bonsai 8B
   Q1 as an experimental low-memory comparison on an M5 Mac with 24 GB of
   memory. Qwen3.8-27B and Bonsai 27B remain later benchmark candidates rather
   than initial defaults.
2. Use llama.cpp as the common first engine for macOS Metal and later Jetson
   CUDA. Pin engine and model provenance. Runtime verifies installed artifacts;
   the model provider owns start, health, limits, streaming, cancellation,
   shutdown, restart, and orphan cleanup. Consider MLX only if the common
   engine path proves insufficient.
3. Prove hosted inference locally. Use the current OpenAI-compatible Chat
   Completions seam where it conforms for OpenRouter, Venice, and xAI/Grok. Add
   one provider-internal OpenAI Responses API adapter.
4. Prove optional sharing of the accepted Mac local model with another Runtime.
   This requires signed offer and grant admission plus bounded Carrier ingress;
   hosted credentials and a Jetson deployment are separate acceptance tracks.
5. Prove hosted sharing only after upstream terms and resale policy permit it,
   with owner-enforced cost and rate limits. Later, prove a full destination
   Runtime on Jetson before considering a smaller provider host.

Installed acceptance covers Brave inference, ordered streaming, reconnect,
cancellation, one terminal result, restart, engine crash and orphan cleanup,
secret and endpoint redaction, and explicit paid provider choice. Remote proof
uses two Runtime identities and two principals. It covers:

- signed offer admission and explicit host-owned grant, revoke, expiry, and
  denial before backend dispatch;
- rejection of forged `runtime_binding` fields and isolation of identical
  principal strings issued by different Runtimes;
- scoped create, get, events, and cancel, including access denial for other
  runs and the host Home and workspace;
- prompt and event privacy, bounded capacity, queues, tokens, time, and usage;
- disconnect and reconnect without redispatch, restart with unknown settlement,
  and service withdrawal; and
- consumer consent to prompt transfer, separately from operator consent to
  compute use or credential charges, with honest provenance and backend facts.

These checks extend the existing service offer and model operations rather
than introducing a separate sharing API, store, or journal. A content CID
identifies an immutable package; a signed service offer identifies an available
capability. Model output and tool proposals remain untrusted and cannot
authorize effects.

## Conformance requirements

A conforming model provider must prove:

- no upstream credential or private endpoint reaches the caller;
- caller, session, capability, and model policy are checked before dispatch;
- selection and fallback behavior are explicit and recorded;
- stream events are ordered and reconnectable;
- each request has one terminal state;
- cancellation and unknown settlement are distinguishable;
- reconnecting presentation does not repeat inference or effects;
- model output cannot authorize a Runtime operation; and
- provider and model changes do not change agent identity or authority.

## Related documents

- [Principles](../PRINCIPLES.md)
- [Architecture](ARCHITECTURE.md)
- [Human and agent architecture](AGENT_ARCHITECTURE.md)
- [Interactive Runtime contract](INTERACTIVE_RUNTIME_CONTRACT.md)
- [Current state](../state.md)
