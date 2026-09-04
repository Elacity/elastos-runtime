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
offer backed by a local engine or a hosted API. Local and remote service use are
placements of the same capability, not separate public model APIs, providers,
or journals.

An operator explicitly selects a configured capability for publication under
Runtime policy. The owning Runtime publishes it as an
`elastos.service.offer/v1` service and owns grants, quotas, selection, audit,
and routing. The destination Runtime authorizes each remote request
independently. Carrier authenticates and transports the route that Runtime
selects; it does not grant model authority. Hosted credentials and local model
artifacts stay inside their owning boundaries.

The owning Runtime DID signs the service offer, which names the admitted
provider capability and contains only bounded capability and policy facts. The
provider identity remains an internal execution binding. Backend URLs,
credentials, process details, and topology stay inside the model provider. A
model artifact is separate immutable content. Its canonical package identity
is the CID of the complete manifest-and-payload closure. Engine, component, and
payload hashes are verification facts rather than package identities. Runtime
installs the package through the content provider path described in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

A hosted web API is a provider-internal HTTPS interoperability edge on the
Runtime that owns the credential. Mac to local model provider to hosted API is
local configured use. Mac Runtime to Carrier to a Jetson or seed Runtime, then
to that Runtime's model provider and backend, is service use. Ordinary capsules
see only typed `elastos://model/*` resources. Publishing a paid hosted offer
requires operator-owned quota, accounting, and data-policy facts.

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

## Staged delivery path

Implement and prove the path in this order:

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
   one provider-internal Responses adapter for OpenAI/Codex-class backends.
4. Add optional service publication after local lifecycle and hosted paths pass.
   Publish only an operator-selected configured capability with a signed offer
   and principal-scoped grant.
5. Prove remote inference with a full Runtime on Jetson, then publish a signed
   service offer and route the granted operation through Carrier. Design a
   smaller provider host only after the full Runtime path establishes its need.

Installed acceptance covers Brave inference, ordered streaming, reconnect,
cancellation, one terminal result, restart, engine crash and orphan cleanup,
secret and endpoint redaction, and explicit paid provider choice. Remote proof
also covers disconnect and reconnect. Model output and tool proposals remain
untrusted and cannot authorize effects.

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
