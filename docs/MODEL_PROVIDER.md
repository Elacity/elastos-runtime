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
Runtime stores hosted credentials in its owner-only secret store, selects the
approved HTTPS destination, and brokers each hosted effect. The confined model
provider owns protocol adaptation, request serialization, streaming,
rate-limit translation, and provider-specific error handling. The model backend
supplies output; it does not gain Runtime authority.

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
| Hosted API | Home owner creates named hosted instances on the model-provider capsule. Default is private. | Owner-funded share after explicit Share, grant, and exact-model terms check |

Both consumption paths use the existing model operations and service-offer
contract. Carrier admits the `model` target through the destination Runtime's
remote-model authorization path. The destination verifies the authenticated
source endpoint and service grant, then constructs its own local provider binding
for the consumer principal, capsule and run. `RuntimeCreateBinding` and
`RuntimeAccessBinding` remain local-channel authority records. Installed
acceptance and remaining remote-service work are recorded in [state.md](../state.md).

An operator explicitly selects a configured capability for publication under
Runtime policy. The owning Runtime publishes it as an
`elastos.service.offer/v1` service and owns grants, quotas, selection, audit,
and routing. For remote model work, the
destination Runtime verifies the signed offer and grant, then maps the
authenticated source Runtime and consumer principal, capsule, and run into
destination-owned authority. Adding an allowlist entry alone cannot establish
this binding. Carrier authenticates and transports the route that Runtime
selects; the destination grants model authority. Publication advertises the
selected capability. Destination-owned grants authorize its use. The host Home,
workspace, and other runs retain their separate access rules. Hosted credentials
stay in the owning Runtime's secret store; local model artifacts stay in their
owning boundaries.

The owning Runtime DID signs the service offer, which names the admitted
provider capability and contains only bounded capability and policy facts. The
provider identity remains an internal execution binding. Hosted URLs and
credentials stay in Runtime's private route; provider process details and
topology stay behind the model-provider contract. A
model artifact is separate immutable content. Its canonical package identity
is the CID of the complete manifest-and-payload closure. Engine, component, and
payload hashes are verification facts rather than package identities. Runtime
prepares and admits the package through the Content provider path in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

A hosted web API is a Runtime-owned HTTPS interoperability edge on the Home
that stores the credential. Runtime checks the exact owner route and sends the
selected destination and credential through its broker. The installed Mac path
confines the provider's direct external sockets; Linux product confinement
still needs proof. The temporary Mac operator route
bypasses normal Inbox decision and grant checks when enabled. Runtime still
selects its permitted destination and credential, and the owner can End that
route. Permanent SEC1 acceptance remains open.

Mac Runtime to Carrier to a Jetson or seed Runtime, then
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

For a remote create, Runtime can return a request-bound
`elastos.model.invocation-refusal/v1` assertion. It states that this invocation
stopped before provider dispatch. The destination emits it, the consumer checks
the exact request, offer, input hash and remote binding, and the gateway returns
HTTP 409. Assistant accepts it only for a fresh remote submission. A retained or
pending request, provider error, malformed assertion or lost reply keeps its
recovery or unknown status. Carrier tries another address after connection
failure, but stops after a model create invocation error. This assertion settles
the current invocation only; it does not settle earlier uncertain attempts.

## Configuration examples

Hosted gateways such as OpenRouter and local engines such as llama.cpp or
Ollama are provider implementations, not architecture. Their model identifiers
and availability can change independently of ElastOS.

The owner of this Home enters hosted credentials in Home Settings. Assistant
and Marketplace open that Settings tab. The Home stores the key in the
existing owner-only model-provider config. After an explicit run, the
selected hosted processor receives the prompt. Product UI names the Home
that stores the key, the processor that receives prompts, the requested
selector, the resolved model when known, and whether explicit fallback is
enabled. Assistant keeps one model selector. The composer placeholder is
Message Assistant. The trigger shows the user-chosen instance name. Each
selector row shows that name and a short route subtitle. Expanded detail
names the processor, prompt destination, payer, limits, privacy, and
availability. Cost appears in a completed run receipt when the backend
reports it. The UI never shows the secret.

Runtime validates the credential separately from a consented paid test.
Validate requires Runtime-owned HTTPS authority to the pinned public host.
The temporary Mac operator route has the bypass and End behavior described
above. The Home can also use an owner-scoped
`providers/model-provider/validate-fixtures.json`. That file, when present,
may bind validate HTTP only to `http://127.0.0.1` with an explicit port.
Save stays private and publishes no service offer. A later Share action uses
existing Marketplace, Services, grants, Carrier routing, and run journals.
Services summary reports `share_enabled` on hosted cards. That field matches
the stored share state. The key stays on the provider Home. Consumers see the
intermediary Home, the selected upstream processor (OpenRouter or Venice), and
the payer. One
connection budget covers that provider's private and shared offers, including
unresolved spend. Provider-side key and account caps cover activity outside
this Runtime. Qualify the exact model and provider terms before Share.
OpenRouter Terms 5.1–5.2 and Venice TOS 7.3 End User API terms are starting
points, not blanket model qualification. Catalogues, browser persistence,
logs, and API responses omit keys and private URLs. The product surface is
the generic model-provider capsule with repeatable owner-bound instances.
Each instance has a stable offer identity, display name, adapter, selected
model, Runtime secret reference, privacy and share policy, limits, and
lifecycle. One Home can keep several instances of the same provider, for
example Jev via OpenRouter, private DeepSeek via OpenRouter, and shared
Venice. Private is the default. Changing or disconnecting one instance leaves
the others. Existing `model:openrouter` and `model:venice` offers migrate into
named instances without losing secrets.

The on-disk operator form remains `providers/model-provider/config.json` with
mode 0600 under parent mode 0700. Secrets live in Runtime-owned secret
storage beside that provider root. That file is the provider boundary, not a
user-facing editor. Terminal, JSON, and key-file workarounds are not the
product path. System supplies generic installed-capsule and secret
management. The capsule UI is Add hosted model, Name, Provider, API key,
Model, Test, and Save, then Use in Assistant, Share as service, Edit, and
Disconnect. An authenticated edit with a blank key retains the stored key of
that same provider instance. A new instance requires a key. Save retries keep
one instance identity, including while activation is pending.
An exact key-only update takes effect for the next request. A running request
retains its original credential and journal binding. A failed provider refresh
returns a pending state after saving configuration; the Home must retry activation
or restart before claiming that configuration is active.

For Decisions, Save binds the exact validated catalog entry's canonical model
ID to the offer revision. This binding is part of the execution hash. The
adapter checks the response against that ID before projecting `output.model`
as the selected contract model. The terminal backend report retains the actual
provider-reported canonical ID and accounting. These are provider reports,
not attestations of upstream computation. Missing or changed identity fails
validation; model prefixes and date suffixes do not establish equivalence.

Canonical architecture documents do not freeze a commercial model name or
claim that a catalog entry will remain available.

Hosted configuration records privacy policy, cost and rate limits, requested
selector, resolved model when the backend reports it, and explicit fallback
policy. The secret stays in Runtime-owned storage on the provider Home. It
does not enter catalogues, offers, logs, or a consumer Home.

System Models owns the Approval Lens evaluator selection by saved instance ID.
Renaming an instance preserves the selection. If that instance becomes
unavailable, Runtime keeps human review required and Inbox remains the decision
surface. Another configured evaluator becomes active only through an explicit
selection.

The labelled sample evaluates a fixed fictional weather request through the
same Decisions contract and provider journal. The selected processor receives
the sample and the Home pays for the evaluation. Advice changes no permission
and dispatches no downstream weather request. A durable receipt binds the
sample to its principal and evaluator; later checks read that same run. Lost
responses or missing journal state retain unknown acceptance instead of
dispatching another paid evaluation.

## Local content selection and retention

Runtime verifies bounded signed catalogue metadata, prepares content, checks the
complete package closure and binds an admitted artifact to a compatible model
offer. Marketplace owns discovery and details; System owns model configuration
and storage controls. Assistant selects an exact admitted or granted offer and
keeps its typed run lifecycle. These surfaces share Runtime state.

Marketplace separates verified local content, exact granted shared models, and
contact access opportunities. The last group opens the existing Services card;
the person requests access there and the provider Home decides in Inbox. After
approval, Marketplace reads grant-scoped model offers and hands the selected
offer to Assistant. Provider Home, processor, prompt destination, payer and
reported limits use the same facts as Assistant. Discovery gives Marketplace
read authority; Services and Runtime retain grant and inference authority.

The installed small-model journey and its exact evidence scope are recorded in
[state.md](../state.md). Acquisition, readiness, inference and warm reuse are
separate checks. A local pin establishes a retained local copy; it does not
establish global availability or exclusive custody.

Selecting a model may ask Runtime to prepare it under current authority. The
person sees availability, Preparing/progress, Ready or actionable failure,
offline and incompatibility states. Preparation can be cancelled or retried.
Use records the caller's Keep claim with the new preparation reservation.
Opening an admitted model or selecting it explicitly in Assistant waits for a
matching Keep acknowledgement; restoring a workspace reads existing state.
Assistant requires a verified catalog mapping for admitted local model selection.
System owns explicit removal. It checks other people's claims before withdrawing
the offer, preserves the caller's Keep on failure, and releases that claim only
after successful removal. Recovery uses the recorded Remove caller. Active-run
protection, recoverability evidence and warning/consent for possible sole-copy
loss remain part of the removal contract. Installed proof covers a new pending
Use claim and idempotent Open/selection of an already-kept SmolLM2 model, including
reload and preserved model files. Removal failure and recovery cases use bounded
source fixtures. Remote inference is a separately selected service. The same inventory distinguishes retention, admission and readiness,
while catalog identity stays visible. A local pin alone
proves neither trust nor engine readiness. There is no user GGUF download,
file-picker or private
path configuration step in this product flow.

Runtime validates signed publisher/catalog and package facts, resource policy,
engine compatibility and content integrity before deriving a private verified
artifact descriptor for the model provider. Source supports operator offers and
admitted-content binding through the existing ProviderRegistry and run journal,
while preserving other configured capabilities. An idle-safe provider refresh
can expose the newly admitted local offer; a busy
or unknown-settlement run prevents destructive reconfiguration and removal.
Content preparation status is distinct from a dispatched model run.

Existing `offers_list`, `runs_create`, `runs_get`, `runs_events` and `runs_cancel`
remain the inference contract. Runtime binds the selected package and offer to
the exact admitted descriptor; the provider revalidates it before inference.
Preparation failure never changes the selected model to an available substitute.
Drafts and existing runs survive selection, progress, cancel, retry and restart.
The ordinary explicit Send/run action dispatches once only when that selected
model and provider are ready.

Bounded transfer and manifest/inventory limits are in
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).
The current execution queue and proof dependencies are in
[TASKS Now](../TASKS.md#now).
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
3. Deliver owner-bound hosted instances through the existing model-provider
   capsule and the OpenAI-compatible Chat Completions seam. OpenRouter and
   Venice remain required adapters. A user can create several named instances
   of the same provider. Use the seam only where official docs confirm the
   request. Private setup publishes no offer.
   Prove a live authorized private hosted run for each provider after the
   owner enters that provider's key in Home. Prove the existing
   provider-internal OpenAI Responses API adapter separately. xAI/Grok stays
   later.
4. Prove optional sharing of the accepted Mac local model with another Runtime.
   This requires signed offer and grant admission plus bounded Carrier ingress.
   A Jetson deployment is a separate acceptance track.
5. After each private connection exists, add an explicit owner-funded Share of
   that exact provider and model through existing Services and Marketplace.
   Qualify OpenRouter Terms 5.1–5.2 or Venice TOS 7.3, plus the selected model
   terms, before Share. Keep one connection budget across that provider's
   private and shared offers, with per-consumer limits. Commercial billing,
   staking, and wider providers stay Later. Later, prove a full destination
   Runtime on Jetson. Jev stays on TypeSafe/OpenRouter until a separate proof
   shows it on another processor.

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
