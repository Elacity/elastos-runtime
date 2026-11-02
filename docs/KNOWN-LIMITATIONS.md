# Known limitations

This list is for the ElastOS 0.8.0 tree. It records the "not done yet" items in
`elastos/CHANGELOG.md`, the open findings in `SECURITY.md`, and the
operating-system differences the code shows. There has been no external audit.

## Not done yet

`elastos/CHANGELOG.md`, section `[0.8.0]`, records this isolation work as
planned for 0.8.1:

- The seed does not yet run under its own account
  ([#174](https://github.com/Elacity/elastos-runtime/issues/174)).
- Home can still get every app's permissions.
- Providers run as normal programs with the account's rights. Only the model
  provider is partly limited. The code for that split is under
  [Isolation and confinement by OS](#isolation-and-confinement-by-os).
- Approvals are not yet shown on a screen that only the Runtime controls
  (S1 and S2 of
  [#173](https://github.com/Elacity/elastos-runtime/issues/173)). The shipped
  approval path is the shell allow-list in
  [the threat model](THREAT-MODEL.md).
- Importing a Recovery Kit does not yet ask for the passkey again
  ([#403](https://github.com/Elacity/elastos-runtime/issues/403)).
- On Linux with KVM, the Browser network helper is installed with `sudo` and
  the installer does not compare a hash
  ([#406](https://github.com/Elacity/elastos-runtime/issues/406)).
  `prepare_browser_linux_host` in `scripts/install.sh` runs
  `sudo install` of `scripts/browser-vm-linux-network.py` to
  `/usr/local/lib/elastos/browser-vm-linux-network.py` when `/dev/kvm` exists
  and the helper file is still present. That copy has no checksum comparison.

## Browser on Linux

Browser is offered only on Apple Silicon Macs. `browser_available` in
`elastos/crates/elastos-server/src/setup/browser_config.rs` returns true only
for `darwin-arm64`. On every other platform, setup writes an empty Browser
engine list and prints "Browser is not available on this computer yet."
`docs/GETTING_STARTED.md` and `docs/INSTALL.md` say the same for 0.8.0, and the
0.8.0 changelog lists "Browser runs only on Apple Silicon Macs." Linux crosvm
launcher files remain in the tree. They are not the 0.8.0 Home Browser path.

## Isolation and confinement by OS

Providers other than the model provider start through `ProviderBridge::spawn`
in `elastos/crates/elastos-runtime/src/provider/bridge.rs`. That path starts a
normal child process with the Runtime user's rights. The shell helper is also
a native host process (`SECURITY.md`).

The model provider is the exception, and the boundary depends on the OS
(`elastos/crates/elastos-server/src/server_infra.rs`):

- **macOS.** `spawn_confined_model_with_engine_bundle` applies a Seatbelt
  profile. The profile allows default, then denies outbound network and file
  read and write, then allows named paths such as `/System`, `/usr/lib`, and
  the admitted model files (`bridge.rs`).
- **Linux x86_64 and Linux aarch64.** `spawn_confined_model_linux` installs
  the filter in `elastos/crates/elastos-runtime/src/provider/linux_model_seccomp.rs`.
  The provider and its descendants may create Unix sockets. Creating an
  Internet socket is refused. The filter does not apply a file-path policy.
  The process still runs as the Runtime user.
- **Other Linux architectures.** `install_on_command` returns
  "Linux model socket confinement is unavailable on this architecture", so
  this confined start fails.
- **Any other OS.** The server uses plain `ProviderBridge::spawn`, with no
  Seatbelt profile and no socket filter.

WASM component capsules run inside the Runtime process on every OS
(`elastos/crates/elastos-compute/src/providers/component.rs`). The Wasmtime
store limit is 128 MiB, and the fuel budget is 100,000,000. The linear
memories are separate. The operating-system process is the same process that
holds the Runtime keys (`SECURITY.md`).

The web Terminal is off by default. When it is enabled, it runs commands with
the host user's rights (`SECURITY.md`).

## Open findings

These four findings are open in `SECURITY.md`. The summaries below match that
file and the cited code.

### Capability state and key rotation are not restart safe

Severity: medium. Files:
`elastos/crates/elastos-runtime/src/capability/manager.rs`,
`elastos/crates/elastos-server/src/security_cmd.rs`.

Creating a signing key logs a persistence error and continues with the
in-memory key (`generate_and_persist_key`). Reading a bad key file does the
same (`load_or_generate`). `rotate_signing_key` logs a write error and still
switches to the new in-memory key. `elastos emergency rotate` builds a fresh
`CapabilityStore::new()` and then prints "Signing key rotated." That store is
not the durable store. The running server loads
`CapabilityStore::with_persistence` at `{data_dir}/capability_store`
(`serve_capability_store` in
`elastos/crates/elastos-server/src/server_infra.rs`). Use counts for limited
tokens live in that process's memory (`try_use_token` in
`elastos/crates/elastos-runtime/src/capability/store.rs`).

### Host-plane Carrier providers use Runtime admission

Severity: medium. File:
`elastos/crates/elastos-server/src/carrier_service.rs`.

Host-plane Carrier service requests use a raw operation and path envelope.
That envelope has no capsule capability-token field. The Runtime admits this
provider class. The envelope is not a separate capsule grant.

### I/O bridge line reads are not bounded while reading

File: `elastos/crates/elastos-runtime/src/handler/io_bridge.rs`.

The I/O bridge rejects a complete line above 1 MiB. The line readers allocate
before that check.

### Whole-object Carrier fetch replies are held in memory

File: `elastos/crates/elastos-server/src/carrier.rs`. Open as
[#341](https://github.com/Elacity/elastos-runtime/issues/341).

A whole-object content fetch arrives as one line. Reading stops once the line
passes one 200 MiB file encoded as base64 (about 268 MiB). Only two such
fetches run at once, and one per peer.
