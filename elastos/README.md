# ElastOS Workspace

This workspace contains the runtime crates and core capsules that back the current public ElastOS flow.
For install and product use, start with the repository root [README.md](../README.md).

## Overview

- one host binary: `elastos`
- signed install, setup, and update flow
- capsule-native runtime with explicit capabilities
- shared protocol crates under `crates/` and core capsules under `capsules/`

## Prerequisites

### Required
- Rust 1.89+ (2021 edition)
- Linux
- Git

### Optional
- KVM (`/dev/kvm`) for microVM capsules
- crosvm + `vmlinux` for supervisor-managed microVM launches
- Node.js 18+ for JavaScript SDK work

## Build

```bash
git clone https://github.com/Elacity/elastos-runtime.git
cd elastos-runtime/elastos
cargo build --workspace --release
./target/release/elastos --help
```

If you use `rustup`, the repository root includes a `rust-toolchain.toml` pin for the expected toolchain.

## Developer Flow

```bash
# From the workspace root
cargo test --workspace

# Explicit runtime for power-user commands
./target/release/elastos serve
```

`elastos run` is the explicit path/CID launch surface. It is not part of the normal install → setup → PC2 user path, and it expects a built capsule directory or CID plus an already-running operator runtime.

## Project Structure

```text
elastos/
├── crates/
│   ├── elastos-common/
│   ├── elastos-compute/
│   ├── elastos-crosvm/
│   ├── elastos-guest/
│   ├── elastos-identity/
│   ├── elastos-namespace/
│   ├── elastos-runtime/
│   ├── elastos-server/
│   ├── elastos-storage/
│   └── elastos-tls/
├── capsules/
│   ├── localhost-provider/
│   └── shell/
└── tools/
    └── vsock-proxy/
```

## Testing

```bash
cargo test --workspace
cargo test -p elastos-runtime
```

## More

- [../docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md)
- [../docs/COMMAND_MATRIX.md](../docs/COMMAND_MATRIX.md)
- [CHANGELOG.md](CHANGELOG.md)
