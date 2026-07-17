# Capsule Templates

Copy one template to `capsules/<name>` and rename every `example-*` identifier.
Do not combine templates unless the resulting package still has one role and
one authority boundary.

For the smallest Component scaffold, `elastos init <name>` generates the
same no-WASI manifest shape. Replace its `local-development` author before
publishing. Use these repository templates when adding a web projection,
viewer/content pair, or provider contract.

- `component-app`: executable no-WASI Component using the ElastOS Bus.
- `web-app`: browser projection over app-scoped Runtime facts and intents.
- `viewer-content`: matched viewer and content manifests showing truthful
  accepted-content declarations.
- `provider-contract`: provider manifest contract. It is not a provider
  implementation and must not become active without Runtime registration,
  capability mapping, denial tests, and audit tests.

Read [Capsule Authoring](../../docs/CAPSULE_AUTHORING.md) before using a
template. Run `node scripts/check-capsule-templates.mjs` after editing them.
