# Hello capsule

`hello` is a manual, standalone example of a portable Rust capsule. It is an
ElastOS Component, so one `hello.component.wasm` artifact can run on every host
that provides the `elastos.component/v1` Runtime ABI.

This directory stays outside `capsules/`, `elastos/`, `components.json`, the
ElastOS Cargo workspace, and all CI, setup, release, and deployment lists. Run
its commands explicitly when you want to use the example.

## Package shape

| Path | Owner |
| --- | --- |
| `capsule.json` | Capsule identity, role, execution ABI, entrypoint, and resource ceiling |
| `src/lib.rs` | Rust guest code and exported `lifecycle.run` function |
| `Cargo.toml` and `Cargo.lock` | Reproducible Rust build input |
| `hello.component.wasm` | Generated portable artifact selected by the manifest |
| `build.sh` | Manual wrapper around the repository Component builder |
| `verify.sh` | Manual test, lint, manifest, and artifact verification |

The generated artifact and Cargo target directory are ignored by Git. Build
them locally before running the capsule.

## Build

From this directory:

```bash
rustup target add wasm32-unknown-unknown
./build.sh
```

The builder first compiles a core WebAssembly module for
`wasm32-unknown-unknown`. It then converts that module to
`hello.component.wasm`, which implements the checked-in `elastos:bus@v1` WIT
contract.

This is host portability, not one native binary for every operating system. A
Component always uses the WebAssembly target. Runtime supplies the host-specific
execution adapter.

## Run

`elastos run` currently accepts a capsule directory or a CID. It does not need
a filename extension for this local example.

Start the operator Runtime in one terminal:

```bash
./elastos/target/release/elastos serve
```

Then run the built capsule from another terminal:

```bash
./elastos/target/release/elastos run "$PWD/examples/hello"
```

When your current directory is `examples/hello`, use:

```bash
../../elastos/target/release/elastos run "$PWD"
```

The successful path ends with:

```text
[run] Component capsule 'hello' exited
```

The Component ABI has no stdout function. The example therefore demonstrates
successful lifecycle entry, Runtime fact access, and clean exit. The host
Runtime prints the visible result.

## Verify manually

```bash
./verify.sh
```

This command is intentionally absent from repository automation.

## Adapt the shape

The manifest-selected payload defines the capsule substrate. Keep one role and
one execution contract per capsule:

| Desired capsule | Main change |
| --- | --- |
| Component app | Keep this Rust source and `elastos.component/v1` manifest |
| Web projection | Use `execution: web-projection` and a browser entrypoint |
| MicroVM | Use `type: microvm` and package a Linux `rootfs.ext4` |
| Data | Use `role: content`, `type: data`, and name the inert data entrypoint |
| Provider | Use `role: provider`, declare one `provides` namespace and complete authority metadata |

A single artifact cannot act as every capsule kind. The manifest, payload, and
Runtime adapter must describe the same execution boundary.
