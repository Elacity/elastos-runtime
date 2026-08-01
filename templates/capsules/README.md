# Capsule templates

Copy one template to `capsules/<name>` and rename every `example-*` identifier.
Do not combine templates unless the resulting package still has one role and
one authority boundary.

`elastos init <name>` generates the minimal no-WASI Component scaffold. Replace
its `local-development` author before publishing. The checked-in templates also
cover a web projection, a viewer/content pair, and a provider contract.

- [`component-app`](component-app/) is an executable no-WASI Component that
  uses the ElastOS Bus.
- [`web-app`](web-app/) is a web projection over capsule-scoped Runtime facts
  and intents.
- [`viewer-content`](viewer-content/) contains matching viewer and content
  manifests with explicit accepted-content declarations.
- [`provider-contract`](provider-contract/) defines a provider manifest
  contract. Activation requires Runtime registration, capability mapping,
  denial tests, and audit tests.

Read [Capsule Authoring](../../docs/CAPSULE_AUTHORING.md) before using a
template. From the repository root, run
`node scripts/check-capsule-templates.mjs` after editing them.
