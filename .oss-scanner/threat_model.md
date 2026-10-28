# Threat model: ElastOS Runtime

## What this project does and where untrusted input enters

ElastOS is a local-first runtime for Apps and services. People sign in to Home with passkeys. The trusted Runtime
core owns isolation, signature verification, principals, sessions, capability enforcement, object routing and audit.
Capsules run without ambient authority and request each effect through a Runtime capability check. Providers are
separate processes that implement protocol behavior behind typed Runtime contracts. Carrier is the
endpoint-authenticated peer-to-peer transport that Runtime selects below its routing.

[PRINCIPLES.md](../PRINCIPLES.md) and [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md) state the intended security
boundaries. [SECURITY.md](../SECURITY.md) lists the findings we already know about.

The code in scope is:

- The `elastos/` Rust workspace. It builds the `elastos` binary (`crates/elastos-server`), the trusted core
  (`crates/elastos-runtime`), identity and authentication (`crates/elastos-identity`, `crates/elastos-auth`), the
  WASM component host (`crates/elastos-compute`), the microVM adapters (`crates/elastos-crosvm`, `crates/elastos-vz`),
  storage and namespaces (`crates/elastos-storage`, `crates/elastos-namespace`), TLS (`crates/elastos-tls`) and the
  protected-content crates (`crates/elastos-protected-content-*`).
- The native provider capsules under `capsules/*/` that have their own `Cargo.lock`, and
  `elastos/capsules/localhost-provider`.
- The Home and App web projections under `capsules/*/` (HTML and JavaScript that run in the person's browser).

Treat all of the following as attacker-controlled.

**Against the Home gateway** (`elastos gateway`, router in `crates/elastos-server/src/api/gateway.rs`). Hosted Home
puts this gateway on the public Internet through a tunnel.

- Every HTTP request from any network client, before sign-in. This includes the passkey (WebAuthn) registration and
  authentication routes, the EVM and BTC challenge and verify routes, owner registration, recovery, the release and
  install routes, `/.well-known/elastos/*`, `/api/carrier/bootstrap`, the content routes (`/s/:cid`, `/ipfs/:cid`,
  `/content/:cid`, `/artifacts/*`) and the public site.
- The `Host`, `Origin`, `Referer`, `Forwarded` and `X-Forwarded-For` headers. The gateway uses them for relying-party
  scope, origin policy and local-owner decisions.
- Requests that carry a Home launch token (`x-elastos-home-token` header or cookie), from a signed-in principal who
  tries to act as another principal or beyond the token's scope. This includes the WebSocket and SSE routes.

**Against the Runtime API** (`elastos serve`, router in `crates/elastos-server/src/api/server.rs`).

- Every HTTP request. `/api/health` and `/api/docs` are public. `/api/auth/attach` takes the attach secret. All other
  routes need a bearer session, and some need a shell session (`api/middleware.rs`).
- A capsule session that tries to reach shell-only routes, capability grant or revoke, audit, supervisor or capsule
  management.
- A cross-origin web page in the person's browser that sends requests to the local Runtime or gateway.

**From a capsule.** A capsule is untrusted code with zero ambient authority.

- A WASM component calling the `elastos:bus@v1` host interface (`elastos/wit/`, host side in
  `crates/elastos-compute/src/providers/component.rs` and `crates/elastos-server/src/resource_bridge.rs`).
- A microVM guest writing to its resource-bridge socket and virtio console.
- A web projection posting messages to the Home shell and calling its narrow Runtime adapter.
- A capsule manifest, its declared resources and its requested capabilities.

**From the network peer side.**

- Every Carrier stream, gossip message, ticket and manifest from a remote peer (`crates/elastos-server/src/carrier*.rs`).
  Carrier authenticates the endpoint only. It does not prove message authorship.
- Collaboration envelopes and signed profile documents (`collaboration_protocol.rs`, `collaboration_core.rs`,
  `collaboration_profile_authority.rs`).
- Release heads, update manifests, `components.json` entries, capsule packages and content fetched by CID. A CID proves
  integrity of bytes only. Publisher identity comes from signature verification against publisher policy.

**Against provider processes.** Providers read JSON lines on stdin from Runtime (`crates/elastos-runtime/src/provider/`).
The request fields that come from a capsule or a remote peer are attacker-controlled, even though Runtime frames the
request.

## Components that matter most / least

Most important, in this order:

1. **Authentication and sessions.** Passkey verification (`crates/elastos-identity/src/webauthn.rs`), SIWE and wallet
   verification and recovery (`crates/elastos-auth/src/lib.rs`), gateway auth and owner registration
   (`crates/elastos-server/src/api/auth_gateway.rs`, `src/auth.rs`), launch tokens and origin policy
   (`api/gateway_home_token.rs`, `api/gateway_origin.rs`, `api/gateway_passkey_step_up.rs`), and the attach handler
   (`api/handlers/attach.rs`).
2. **Capability tokens.** Minting, delegation, the ordered validation checks, epochs and revocation
   (`crates/elastos-runtime/src/capability/`), and every site that mints or checks a token
   (`api/handlers/capability.rs`, `api/handlers/provider.rs`, `api/handlers/namespace.rs`, `resource_bridge.rs`).
3. **Capsule isolation.** The wasmtime component host, fuel and store limits, the Bus host functions, resource-bridge
   authorization (`manifest_allows_resource`, `authorize_and_dispatch_resource_invoke`), the microVM configuration and
   provider process spawning and sandboxing (`crates/elastos-runtime/src/provider/bridge.rs`).
4. **Keys and protected content.** The Runtime signing key, the device DID key (`crates/elastos-identity/src/store.rs`),
   and the providers that hold or release secrets: `capsules/key-provider`, `capsules/decrypt-provider`,
   `capsules/custody-provider`, `capsules/protected-content-protect-provider`,
   `capsules/protected-content-decrypt-provider`, `capsules/wallet-provider`, `capsules/did-provider`,
   `capsules/drm-provider` and `capsules/rights-provider`. Raw content encryption keys must stay inside the
   protected-content providers.
5. **Storage and paths.** Principal roots (`localhost://Users/<principal-root>`), `elastos/capsules/localhost-provider`,
   `api/handlers/storage.rs`, `crates/elastos-storage`, `crates/elastos-namespace`, and the gateway file-serving code
   (`gateway_site.rs`, `browser_capsules.rs`, `viewer_gateway.rs`, `gateway_provider_proxy.rs`).
6. **Carrier and collaboration.** Stream and gossip parsing, signed envelope and profile verification, bootstrap peers.
7. **Release and artifact trust.** Checksum verification (`crates/elastos-server/src/setup.rs`, `binaries.rs`), capsule
   signature verification (`crates/elastos-runtime/src/signature/`, `crates/elastos-server/src/runtime.rs`) and update
   verification (`update.rs`, `crypto.rs`, `trust_cmd.rs`).
8. **Home and App web projections.** XSS or message-origin flaws in the Home shell that let one App act as Home or as
   another App.

Other notes on scope:

- Network-facing provider capsules (`net-provider`, `tunnel-provider`, `exit-provider`, `ipfs-provider`,
  `chain-provider`, `model-provider`, `media-provider`, `availability-provider`) are in scope. A flaw that lets a capsule
  use them without a matching grant is high.
- Code under `elastos/tools/` (Browser engine helpers and the componentize tool) is lower priority. Report it only when
  Runtime or a capsule can reach it.
- `deploy/`, `scripts/`, `website/`, `docs/`, `examples/`, `templates/` and `.github/` are out of scope. So are the
  game capsules (`gba-*`) and the vendored `capsules/gba-emulator/browser/mgba.js`.
- Third-party crates have their own security processes. Report a flaw in a dependency only when ElastOS's use of it
  makes it reachable, and show the path.

## How to exercise it

The checkout is in `/src`. Every crate in every lockfile is fetched and `CARGO_NET_OFFLINE=true` is set, so rebuilds
with other features or profiles work offline. Builds use the dev profile, so debug assertions and overflow checks are
on. Intermediate artifacts are in `/src/target-build` (see `.cargo/config.toml`). Final binaries are in each
workspace's own `target/debug`.

**Tests**

- The Runtime workspace: `cd /src/elastos && cargo test --workspace --no-fail-fast`. The image sets
  `ELASTOS_TEST_PROTECT_PROVIDER_BIN`, `ELASTOS_TEST_DECRYPT_PROVIDER_BIN` and `ELASTOS_TEST_CUSTODY_PROVIDER_BIN` to
  the provider binaries that some integration tests start.
- Each capsule: `cd /src/capsules/<name> && cargo test`.
- Tests that need the network, a running IPFS node, Kubo, a model or KVM are `#[ignore]`d. This image has no KVM, so
  crosvm and Browser VM paths fail closed. `crates/elastos-vz` needs macOS on Apple silicon.
- Two groups of tests depend on the machine, not on the code. Storage, model, publication and Browser image paths keep
  a free-space reserve, so their tests fail on a nearly full volume (errors that say "not enough free space" or "more
  free space"). Tests that inject a write failure through file permissions pass only as a non-root user, because root
  ignores those permissions. Treat these failures as environment results, not findings. Run the tests as a non-root
  user on a volume with free space to get a clean baseline.
- Gateway handlers can be driven in process with `tower::ServiceExt::oneshot` from the helpers in
  `crates/elastos-server/src/api/gateway_tests/` (`mod.rs` has `test_state`, `support_runtime.rs` has
  `start_fake_runtime`). A new test there is the preferred reproducer for a gateway finding.

**Running a Runtime**

The Runtime verifies every staged component against `components.json`, so stage the localhost-provider first:

```sh
export HOME=/tmp/elastos-home XDG_DATA_HOME=/tmp/elastos-home/.local/share
mkdir -p "$XDG_DATA_HOME/elastos/bin"
cp /src/elastos/target/debug/localhost-provider "$XDG_DATA_HOME/elastos/bin/"
sum=$(sha256sum "$XDG_DATA_HOME/elastos/bin/localhost-provider" | cut -d' ' -f1)
jq --arg c "sha256:$sum" '.external["localhost-provider"].platforms["linux-amd64"].checksum = $c' \
  /src/components.json > "$XDG_DATA_HOME/elastos/components.json"
/src/elastos/target/debug/elastos serve --addr 127.0.0.1:3000 &
curl -s http://127.0.0.1:3000/api/health
```

To get a shell session, read `attach_secret` from `$XDG_DATA_HOME/elastos/runtime-coords.json` and send
`POST /api/auth/attach` with `{"secret": "<attach_secret>", "scope": "shell"}`. `elastos gateway` serves Home with the
same data directory. `elastos identity show` and `elastos init` work against an isolated `HOME`.

Debug builds add `POST /api/test/create-session`, which mints sessions without authentication. It exists only for
tests. Do not use it as the entry point of a finding. When a finding depends on the exact release behavior, rebuild
with `cargo build --release -p elastos-server`.

## How you rate severity

A finding counts when an attacker in one of the positions above makes ElastOS break a boundary that PRINCIPLES.md or
docs/ARCHITECTURE.md states. Say which position the attacker holds: a network client of the gateway, a signed-in
principal, a capsule, a Carrier peer, a malicious publisher or content source, or a local user on the same host.
Findings against a release build with the default configuration weigh more than ones that need a non-default option.

- **Critical**
  - Code execution on the host from a network client, a Carrier peer or a capsule. This includes an escape from a WASM
    Component's memory and fuel limits, from a microVM guest, or from the model provider's confinement.
  - Signing in, or getting a session or launch token, as a principal without that principal's passkey or credential.
  - Forging a capability token, or passing validation with a token that should fail any of the checks in
    `capability/manager.rs`.
  - Disclosure of the Runtime signing key, a device DID key, a custody secret, a wallet key or a raw content encryption
    key to anyone outside the component that owns it.
  - Runtime accepting a release, update, component or capsule that the configured publisher did not sign.
- **High**
  - One principal reading or changing another principal's data, sessions, credentials or recovery material.
  - A capsule getting authority beyond its manifest and grants: reaching shell-only routes, another capsule's resources,
    ambient network, or a provider without a matching grant.
  - Path traversal or symlink abuse that reads or writes outside the caller's root.
  - A Carrier peer forging a collaboration message or profile, impersonating a member, or getting past admission.
  - A cross-origin page in the person's browser that performs an authenticated action on the local Runtime or gateway.
  - XSS in the Home shell, or a message-origin flaw that lets an App act as Home.
  - A network client without credentials who crashes, hangs or exhausts the memory or CPU of the gateway or Runtime with
    cheap input.
- **Medium**
  - Denial of service that needs a signed-in session or an admitted Carrier peer and stays bounded to that session.
  - A panic confined to one request, stream or capsule that does not stop the process.
  - Audit events that are missing or can be forged for a security-relevant operation.
  - Metadata leaks, such as principal roots or peer addresses, to parties that the contracts say do not learn them.
  - Secrets written to logs at info level or above.
- **Low**
  - Problems that need a debug build, a test helper, a non-default environment override (for example
    `ELASTOS_SHADOW_MODE` or `ELASTOS_CAPSULE_BIN_DIR`), or a malicious local config file.
  - Problems that need write access to the person's data directory.
  - Panics caused by invalid arguments from the operator's own CLI.

A bug with no security consequence, such as a wrong label, a stale doc or a flaky test, is not a vulnerability. Do not
report it.

## How to report

- Give a reproducer that runs in this image, offline. The best form is a Rust test in the affected crate. For the
  gateway, use the in-process helpers above. Otherwise give commands against a Runtime started as shown above, with the
  exact requests sent and the responses observed. For resource exhaustion, give the input and the measured memory, CPU
  or time.
- Name the attacker position and any configuration that differs from the default.
- Propose a minimal patch with a regression test. Keep unrelated refactoring out of it, and run `cargo fmt` in the
  workspace you change.
- Report one root cause once, and list every path that reaches it.

## Anything to leave alone

- The open findings in SECURITY.md are known: capability state and key rotation that are not restart safe, host-plane
  Carrier providers that use Runtime admission, I/O bridge line reads that are not bounded while reading, and
  whole-object Carrier fetch replies that are not size-bounded. Report a variant only when it reaches a new boundary
  or a higher severity.
- SECURITY.md also states the current isolation boundary. These limits are known and have their own plans:
  - Home can currently obtain every App's capability, so a compromised Home reaches those Apps' authority.
  - WASM Components run inside the Runtime process, with separate linear memories but the same operating system
    process as the Runtime keys.
  - Providers and the trusted shell helper run as native processes with the Runtime user's rights. Only the model
    provider is partly confined.
  - An enabled web Terminal runs commands with the host user's rights. It needs host developer mode and closed guest
    registration.
  - Keys are stored next to the data they protect. The host operator and root can read hosted data, wallet keys and
    recovery material, also while Home is locked.

  A finding that only restates one of these limits is not new. A finding that gets past one of the stated
  conditions is. Examples are a Terminal enabled without developer mode, a WASM Component that reads Runtime memory,
  or an App that gets another App's capability without going through Home.
- The temporary Mac operator model route described in docs/ARCHITECTURE.md bypasses Inbox decision and grant checks
  while an operator enables it. This is known.
- Content routes serve public content by CID by design. A CID proves integrity only, so that fetched content is not
  authenticated is not a finding by itself. Accepting it as trusted without a publisher signature is.
- Carrier endpoint authentication does not prove message authorship. That alone is not a finding. Accepting an
  unsigned or wrongly signed envelope as authored by a member is.
- crosvm and Browser VM paths fail closed without KVM. That failure is expected in this image.
