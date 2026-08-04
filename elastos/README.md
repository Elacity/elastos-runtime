# ElastOS Rust workspace

This directory contains the Cargo workspace for the Runtime libraries, the
`elastos` host binary, and the Rust capsules maintained with them. For
installation and product use, start with the repository
[README](../README.md).

Run Cargo workspace commands from this directory. From the repository root:

```bash
cd elastos
```

When running Cargo from the repository root, pass
`--manifest-path elastos/Cargo.toml`.

## Requirements

- Rust 1.89.0, pinned by the repository's
  [`rust-toolchain.toml`](../rust-toolchain.toml)
- Git

Platform requirements depend on the code path. Linux/crosvm microVM work
requires Linux and KVM. macOS source staging uses VZ; see the
[Mac runbook](../docs/MAC.md).

## Build

```bash
cargo build --workspace --release
./target/release/elastos --help
```

The `elastos-server` package produces the `elastos` binary.

## Navigate the code

- [`Cargo.toml`](Cargo.toml) is the source of truth for workspace members and
  shared Rust settings.
- [`crates/`](crates/) contains the Runtime libraries and the `elastos-server`
  host package.
- [`capsules/`](capsules/) contains the Rust capsules included in this
  workspace.
- [`tools/`](tools/) contains standalone Rust and JavaScript utilities. They are
  not built by `cargo build --workspace`; use each tool's own manifest or
  package file.
- [`wit/`](wit/) contains the checked-in ElastOS Bus interface.

## Testing

```bash
cargo test --workspace
cargo test -p elastos-runtime
```

The repository root also provides `just test`, `just test-crate <crate>`, and
`just verify`.

## Related documentation

- [Architecture](../docs/ARCHITECTURE.md)
- [Command matrix](../docs/COMMAND_MATRIX.md)
- [Changelog](CHANGELOG.md)
