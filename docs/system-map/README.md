# ElastOS system map

This directory is a compact navigation map for people and agents working on
ElastOS. It explains where responsibilities and code live. It does not create
a second architecture, glossary, backlog, or readiness ledger.

## Sources of truth

Read these in order when a statement matters:

1. [`PRINCIPLES.md`](../../PRINCIPLES.md) defines stable product and authority
   constraints.
2. [`ARCHITECTURE.md`](../ARCHITECTURE.md) defines responsibility and trust
   boundaries.
3. [`state.md`](../../state.md) records verified current behavior and known
   limitations.
4. [`TASKS.md`](../../TASKS.md) records open work.
5. [`AGENTS.md`](../../AGENTS.md) defines the operator and agent process.
6. The contract for the touched surface defines its exact interface.

If this map disagrees with one of those files, the source of truth wins and
this map must be corrected.

## Change gate

Before editing a diagram or path map, fill in this short record:

```text
Layer:
Capability:
Hidden detail:
Done-check:
```

`Layer` names the Home, Runtime, capsule, provider, Carrier, or host-adapter
boundary. `Capability` names the exact resource and operation. `Hidden detail`
states what the capsule or projection must not see, such as a host path, peer,
backend, credential, socket, or transport. `Done-check` is one command a new
contributor can run to prove the change.

For this map, the minimum check is `node scripts/system-map-check.mjs` from the
repository root. A code or contract change also needs the narrow test for its
own surface. If those four lines cannot be stated, the proposed edit is not
ready.

## Read this map

- [Layered C4 model](c4.md): system context, containers, components, supporting
  code ownership, trust, identity, dynamic, and deployment views. Each diagram
  is deliberately small enough to read or copy into Mermaid Live on its own.
- [Interactive C4 viewer](viewer.html): a navigable projection of the same
  model. `c4.md` remains canonical; run `node scripts/system-map-check.mjs` from
  the repository root to verify that the viewer is bound to its exact source.
- [Code and product tree](tree.md): stable paths for the major Runtime,
  capsule, provider, Home, collaboration, Browser, and AI surfaces.
- [Human and agent architecture](../AGENT_ARCHITECTURE.md): principal parity,
  Profiles, Agent Host capsules, delegation, and model/effect flows.
- [Private network contract](../PRIVATE_NETWORK.md): signed membership, named
  services, Carrier routes, and optional compatibility adapters.
- [Model provider contract](../MODEL_PROVIDER.md): explicit selection,
  streaming, cancellation, recovery, and terminal results.
- [Consequence-aware effects](../CONSEQUENCE_AWARE_EFFECTS.md): one Runtime
  effect path with operation-specific observation, actuation, settlement, and
  local-safety requirements.

Use the canonical [Glossary](../GLOSSARY.md) for terminology. Use the
[documentation index](../README.md) to find detailed contracts and verification
guides.
