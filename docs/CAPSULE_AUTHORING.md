# Capsule Authoring

This is the practical entry point for adding a capsule. The canonical schema is
`elastos.capsule/v1`; Runtime validation, not this document, is authoritative.

Start from [templates/capsules](../templates/capsules/README.md), then keep the
change inside one role and one authority boundary.

`elastos init <name>` creates a minimal WASM Component that uses the ElastOS Bus, and
`elastos init <name> --type content` creates a Documents content capsule. The
repository templates cover the other roles. Generated `local-development`
publisher metadata must be replaced before publishing.

## Choose One Role

| Role | Use it for | Authority rule |
| --- | --- | --- |
| `app` | A user-facing application | No provider namespace, ambient network, host files, or System backend authority. |
| `shell` | A Home projection | Shell selection and host intents remain Runtime-authorized. A shell is not a provider. |
| `viewer` | An app that opens declared content or files | Declare accepted content in `input_schema.accepts`; never infer compatibility from names. |
| `content` | Portable data opened by a viewer | Use `type=data` and declare exactly one installed viewer. Content does not provide services. |
| `provider` | A Runtime-owned service implementation | Declare one narrow `provides` namespace plus authority operations and audit events. Runtime must register the implementation. |

If the capsule needs raw sockets, DNS, host paths, keys, chain signing, a VM,
or another privileged backend, it is not an ordinary app. Put the effect behind
a provider contract and keep the app on typed intents.

## Choose One Execution Contract

### Components And ElastOS Bus

Use `elastos.component/v1` for executable first-party WASM Components and
`elastos:bus@v1` for their Runtime-mediated authority contract.

- Build a WASM Component for `product-capsule-v1`.
- Import only the interfaces declared by `elastos:bus@v1`.
- Declare the exact `wit_world_sha256` from
  `node scripts/check-elastos-bus-wit.mjs`.
- Do not use WASI, environment variables, preopens, sockets, FIFOs, gateway
  URLs, or host process APIs.
- Invoke effects by resource URI and operation. Runtime selects providers,
  validates capabilities, routes over Carrier when needed, and records audit.
- ElastOS Bus v1 has no stream interface. A later contract version must add one
  explicitly if Runtime stream authorization and lifecycle are implemented.

### Web Projection

Use `elastos.runtime-projection/v1` when the capsule is browser presentation
over Runtime facts and typed intents.

- The browser directory is presentation, not an isolation or authority boundary.
- Launch tokens and app-scoped Runtime routes authorize requests.
- Do not call providers directly or treat same-origin access as permission.
- Off-box effects still cross Runtime and a provider/Carrier path.

### Data

Use `role=content`, `type=data`, and a `viewer` binding. Data capsules do not
declare an executable ABI.

## Manifest Checklist

Every manifest needs:

- stable `name`, `version`, `description`, `author`, `role`, `type`, and
  `entrypoint`;
- one execution contract for executable WASM;
- `interfaces[*].methods[*]` with stable id, plain description, risk, approval,
  audit mode, resource URI, and operation;
- ordinary apps, viewers, and content may declare capsule requirements only;
  external host dependencies belong behind provider contracts;
- viewer compatibility in `input_schema.accepts` and content-side `viewer`;
- narrow resource limits;
- no client-supplied identity, principal, token, provider route, raw host path,
  signature, or Runtime metadata fields.

An interface descriptor is not executable authority. Runtime derives binding
availability and exposes a method as executable only when a canonical handler
and policy path exist.

## Public Language

People see Apps, files, services, permissions, and approvals. Reserve capsule,
projection, schema, provider boundary, capability surface, and Runtime mirror
for explicit technical details. Descriptions should say what the user can do,
not narrate the architecture.

Good: `Play GBA games`.

Bad: `Runtime-owned viewer projection over provider-derived capsule facts`.

## Build And Verify

For a Component capsule under `capsules/<name>`:

```bash
scripts/build-component-capsule.sh capsules/<name>
```

The builder uses locked dependencies, an isolated target directory, and stable
path remapping. Setup and release packaging call the same builder; do not commit
an artifact produced by a persistent capsule `target/` directory.

Minimum gate:

```bash
node scripts/check-capsule-templates.mjs
node scripts/check-elastos-bus-wit.mjs
node scripts/check-first-party-wasi-gate.mjs
python3 scripts/source-home-capsule-inventory-smoke.py
(cd elastos && cargo test -p elastos-common manifest -- --nocapture)
(cd elastos && cargo test -p elastos-server init::tests -- --nocapture)
(cd elastos && cargo test -p elastos-server component_conformance_exercises_bus_authorization_dispatch_and_audit -- --nocapture)
git diff --check
```

Also run the app's focused behavior test and installed artifact parity proof.
Source presence alone does not make a capsule installed, active, launchable, or
working.

Before publishing, replace scaffold values such as `local-development` and
`example-publisher`. The release publisher validates the complete manifest and
rejects missing descriptions, missing authors, and placeholder authors.

## Review Questions

1. Is this one role, or are app and provider responsibilities mixed?
2. Can every requested effect be named as resource, operation, action, gate,
   and audit event?
3. Does the capsule remain correct if the provider is local, remote over
   Carrier, unavailable, or replaced?
4. Can Runtime derive every catalog, viewer, requirement, and executable-binding
   claim from installed manifests and live registrations?
5. Does denial fail closed without hidden fallback or ambient authority?
6. Are state and secrets principal-scoped and outside the immutable artifact?
7. Is the public copy useful without exposing implementation vocabulary?

Deeper references:

- [CAPSULE_MODEL.md](CAPSULE_MODEL.md)
- [CAPSULE_INTERFACE_CONTRACT.md](CAPSULE_INTERFACE_CONTRACT.md)
- [NAMESPACES.md](NAMESPACES.md)
- [CARRIER.md](CARRIER.md)
