# ElastOS Runtime overview

ElastOS is a local-first Runtime for Apps and services. Home is the user-facing
front door. Runtime derives identity and authority, routes typed resource
requests, owns lifecycle and audit, and delegates provider-backed operations to
the provider that owns their semantics.

Executable capsules request typed Runtime resources. Components use
`elastos:bus@v1`; web projections use narrow Runtime adapters. Carrier is an
endpoint-authenticated off-box transport selected below that boundary. It is
not the capsule API and does not by itself prove application-message
authorship.

Use these documents as the sources of truth:

- [Principles](../PRINCIPLES.md): stable implementation constraints.
- [Current state](../state.md): verified behavior, branch status, and known
  limitations.
- [Open work](../TASKS.md): prioritized unfinished work.
- [Architecture](ARCHITECTURE.md): trust boundaries and component ownership.
- [Capsule model](CAPSULE_MODEL.md): package, execution, and isolation model.
- [Capsule authoring](CAPSULE_AUTHORING.md): checked manifest and template path.
- [Namespaces](NAMESPACES.md): rooted local and content identities.
- [Command matrix](COMMAND_MATRIX.md): user, operator, and no-Runtime command
  lanes.
- [Documentation index](README.md): the complete active documentation map.

Release readiness is tracked by the
[source and release checklist](RUNTIME_REPO_USER_STORY_CHECKLIST.md). This overview
does not declare a feature, target, or release accepted.
