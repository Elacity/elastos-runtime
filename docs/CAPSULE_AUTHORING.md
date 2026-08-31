# Capsule authoring

The canonical manifest schema is `elastos.capsule/v1`. The Rust type and
validator in
[`elastos-common/src/manifest.rs`](../elastos/crates/elastos-common/src/manifest.rs)
are authoritative. This guide explains the current authoring path without
turning optional metadata into a false requirement.

Start with `elastos init` for a small Component or content capsule. Use
[`templates/capsules`](../templates/capsules/README.md) for a web projection,
viewer/content pair, or provider contract.

The Component path is implemented and conformance-tested, but no shipped
first-party product App uses it yet. The 18 shipped first-party UI Apps are
`elastos.runtime-projection/v1` web projections. Treat the Component scaffold
as the supported authoring contract, not as evidence of product adoption.

The commands in this guide create capsule source packages. A source package is
an authoring and build input, not a complete signed Digital Capsule. Source
packages may omit `signature` during local development; [state.md](../state.md)
records the current first-party signing gap. The source-home setup copies these
packages into its local data home, and local development may launch them. Their
presence or successful launch does not prove signed distribution, portable
installation, or Runtime admission on another node. A distributable Digital
Capsule requires a complete signed artifact. Each Runtime decides separately
whether to admit it under local trust policy.

## Choose one role

| Role | Use | Boundary |
| --- | --- | --- |
| `app` | User-facing behavior | No provider namespace, ambient network, host files, or system backend authority. |
| `shell` | A Home projection | Runtime owns shell selection and host intents. A shell is not a provider. |
| `viewer` | Opening declared content or files | Put compatibility claims in an interface input schema. Do not infer them from names. |
| `content` | Portable data | Must use `type=data`. A `viewer` binding is optional in the schema; declare one when the data depends on a specific installed viewer. |
| `provider` | A Runtime-owned service implementation | Must declare one `provides` namespace and complete `authority` metadata. Runtime must register the implementation before it is usable. |

Raw sockets, DNS, host paths, keys, signing, virtual machines, and privileged
backends belong behind a provider contract. An ordinary app or viewer requests
a typed operation and does not choose the provider implementation.

## Know which fields are required

Manifest deserialization rejects unknown fields. Requirements then come from
three layers:

| Layer | Required fields or checks |
| --- | --- |
| Base schema | `schema`, `version`, `name`, `role`, `type`, and `entrypoint` must be present. `schema` must be `elastos.capsule/v1`; `version` and `name` must be nonempty; `entrypoint` must be relative and contain no `..`. |
| Role validation | `role=content` requires `type=data`. `role=provider` requires `provides` and `authority`. Non-provider roles cannot declare either provider authority field. |
| Execution validation | Component and web-projection fields must match the exact contracts listed below. |

`description` and `author` are optional at the base-schema layer. So are
`runtime_abi`, `bus_contract`, `wit_world_sha256`, `execution`, `projections`,
`requires`, `capabilities`, `interfaces`, `resources`, `permissions`,
`microvm`, `providers`, `viewer`, and `signature`. Optional does not mean
appropriate for every role; the validator rejects combinations that cross an
authority boundary.

### Component fields

An executable Component uses:

```json
{
  "type": "wasm",
  "runtime_abi": "elastos.component/v1",
  "bus_contract": "elastos:bus@v1",
  "wit_world_sha256": "<current product-capsule-v1 WIT SHA-256>",
  "execution": "component"
}
```

The hash must equal the checked-in `elastos/wit/elastos-bus-v1.wit` bytes.
Compute it with:

```bash
node scripts/check-elastos-bus-wit.mjs
```

The Component imports only `elastos:bus@v1`. It receives no WASI, environment,
filesystem preopens, FIFO, raw socket, or gateway authority. Runtime validates
Bus requests, chooses providers, and records audit events.

### Web-projection fields

A web projection uses:

```json
{
  "type": "wasm",
  "runtime_abi": "elastos.runtime-projection/v1",
  "bus_contract": "elastos.runtime-projection/v1",
  "execution": "web-projection",
  "projections": ["web"]
}
```

Its browser directory is a development projection and presentation surface.
The projection requests effects through narrow, capsule-scoped Runtime
adapters. Both Component and web-projection requests enter Runtime's authority
and routing boundary. Runtime handles core operations directly and sends
provider-backed effects through the provider registry. Each substrate retains
its documented lifecycle and cleanup contract. Launch tokens establish bounded
launch context, but they do not prove principal or session authority by
themselves. Same-origin access grants no authority.

### Data fields

A content capsule uses `role=content` and `type=data`. Its `entrypoint` names
the data entry. It has no executable ABI. Add `viewer` only when one installed
viewer is part of the content contract.

The published CID should identify the complete immutable capsule closure, not
only the entrypoint file. A game capsule declares its viewer and packages the
licensed ROM, artwork, metadata, and notices. A model capsule packages the GGUF
and declares its format, quantization, base model and provenance, resource
requirements, license, and compatible provider interface. Do not put a mutable
web URL in either manifest as the content identity. See
[Content capsule distribution](CONTENT_CAPSULE_DISTRIBUTION.md).

## Keep optional declarations honest

`interfaces` defaults to an empty list. If an interface is declared, it needs
an `id`, `version`, and at least one method. Each method needs `id`, `risk`,
`approval`, and `audit`. Method descriptions, resource URI, operation, input
schema, and output schema are optional in the Rust schema.

For a method advertised as executable, declare a `resource` and `operation`
that map to a real Runtime handler and policy path. The descriptor is
discoverability metadata, not authority. Runtime grants and audit remain
decisive.

`resources` is also optional. When omitted, the manifest parser supplies
`memory_mb=64`, `cpu_shares=100`, and `gpu=false`. Those values do not tune the
current Component runner. The current `elastos init` scaffold explicitly
declares `memory_mb=16`, but that value is also declarative.
`ComponentProvider` currently fixes each activation at:

- 100,000,000 fuel;
- a Wasmtime `memory_size` limit of 128 MiB;
- at most 16 instances, 32 tables, and 4 memories.

These are runtime constants, not manifest negotiation. Do not claim that a
Component's `resources.memory_mb` changes this budget.

`capabilities` and `permissions.storage` are requested upper bounds. They never
grant authority by themselves. Do not put caller identity, principal, tokens,
provider routes, host paths, signatures, or Runtime-generated metadata in an
input schema.

## Create and build

### Small Component scaffold

From the repository root:

```bash
./elastos/target/release/elastos init my-app
cd my-app
cargo build --release --target wasm32-unknown-unknown
cargo run --quiet --manifest-path ../elastos/tools/componentize/Cargo.toml -- \
  target/wasm32-unknown-unknown/release/my_app.wasm \
  my-app.component.wasm
cd ..
```

This is the path printed by the current scaffold. The relative componentizer
path assumes the new directory was created at the repository root.

For a repository capsule with a lockfile, use the reproducible builder:

```bash
scripts/build-component-capsule.sh capsules/<name>
```

The builder uses locked dependencies, an isolated target directory, the
checked-in WIT hash, and stable path remapping. It writes the manifest
entrypoint inside the capsule directory.

### Content scaffold

```bash
./elastos/target/release/elastos init my-docs --type content
```

This creates `my-docs/capsule.json` and `my-docs/README.md`. Add content, then
use `elastos share my-docs` to publish a share or `elastos run my-docs` for the
self-contained data preview.

## Run a Component

Component and microVM `run` commands require the operator runtime. From the
repository root, use two terminals:

```bash
# Terminal 1
./elastos/target/release/elastos serve
```

```bash
# Terminal 2
./elastos/target/release/elastos run my-app
```

The second command fails if the operator runtime is absent. Data capsules are
different: their `run` path starts its own in-process preview. The complete
classification lives in [Command runtime matrix](COMMAND_MATRIX.md).

## Publish with the right gate

The two publish commands enforce different checks:

| Command | Additional requirement |
| --- | --- |
| `elastos publish <path>` | Base and conditional manifest validation, followed by an existence check on the resolved entrypoint path. It does not require `description` or `author`. An empty entrypoint currently resolves to the capsule directory and can pass that check. |
| `elastos publish-release ...` | Every selected manifest needs a nonempty `description` and `author`. It rejects `local-development` and `example-publisher` authors. |

`elastos init` writes `author: "local-development"`, and repository templates
use `example-publisher`. Replace those values before release publication.
Neither publish command turns an unsigned source package into a complete
Digital Capsule. Signed bundle identity, publisher verification, portable
admission, and install receipts remain required distribution work.

## Verify the change

Run the checks that cover the manifest, WIT contract, artifact, and scaffold:

```bash
node scripts/check-capsule-templates.mjs
node scripts/check-elastos-bus-wit.mjs
node scripts/check-first-party-wasi-gate.mjs
python3 scripts/source-home-capsule-inventory-smoke.py
(cd elastos && cargo test -p elastos-common manifest -- --nocapture)
(cd elastos && cargo test -p elastos-server init::tests -- --nocapture)
(cd elastos && cargo test -p elastos-server \
  component_conformance_exercises_bus_authorization_dispatch_and_audit \
  -- --nocapture)
git diff --check
```

Add the capsule's focused behavior test. For an installed path, also compare
the built and installed artifact hashes and exercise the installed command.
Source presence does not prove installation or launch behavior.

## Review boundary

Before review, answer four questions:

1. Does the capsule have one role and one authority boundary?
2. Does every effect cross a named resource, operation, policy gate, and audit
   path?
3. Does denial fail closed without ambient authority or a compatibility
   fallback?
4. Are mutable state and secrets principal-scoped and outside the immutable
   artifact?

Deeper contracts:

- [Capsule model](CAPSULE_MODEL.md)
- [Capsule interface contract](CAPSULE_INTERFACE_CONTRACT.md)
- [Namespaces](NAMESPACES.md)
- [Carrier](CARRIER.md)
