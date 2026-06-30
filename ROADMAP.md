# ElastOS Runtime Roadmap

This file is for direction only.

For active work, see [TASKS.md](TASKS.md).
For current state, see [state.md](state.md).

---

## Mission

Make this repository the trusted local runtime layer of ElastOS:
- execute capsules predictably
- expose one coherent local object model
- use the Carrier/provider plane as the secure communication and content contract for local and off-box effects
- make local and remote effects look like the same capability-scoped plane from the capsule's point of view
- keep release, install, update, share, and site flows boring
- give ElastOS a stable default Home without weakening the runtime model

## Non-Goals

This repo is not the whole SmartWeb stack.
It is not the blockchain/payment layer or the full Carrier/Boson program.
It should integrate with those surfaces without pretending to own them.

This repo does own the runtime and Home contract.
That includes the default local front door, the capsule execution model, and the object/runtime boundaries that future Home surfaces must obey.

## First-Principles Alignment

The PC2 idea is not "no connectivity." It is "no ambient internet."
Capsules should not see raw network, host files, IPFS APIs, databases, or other
capsules as things they can directly reach. They see capability-scoped runtime
operations. The runtime authorizes the request, Carrier or a provider performs
the effect, and audit/provenance records stay attached to the operation.

The correct local/remote abstraction is therefore:

`capsule -> runtime capability -> Carrier/provider plane -> object/service`

HTTP/TLS, browser frames, localhost ports, IPFS, Matrix, Telegram, Nostr, or a
hosted social-network drive can all exist underneath that line. They are adapters
or provider implementations, not the capsule contract and not the product truth.

Out-of-the-box authentication should start with passkeys/WebAuthn because it gives
mainstream users a local, phishing-resistant unlock without making wallet apps,
browser extensions, or chain availability the foundation. A passkey proof binds
to a runtime principal and unlocks short-lived capabilities. It does not replace
DID, wallets, dDRM, or blockchain proof adapters.

Automated agents should not borrow human cookies or automate a person's real
passkey. Development and CI can use browser-supported WebAuthn virtual
authenticators to exercise the same Home passkey ceremonies. Production agents
should instead be explicit delegated principals with their own keys, short-lived
capability grants, audit, and human approval for high-risk scopes.

The default Home rule is simple: the first passkey created on a runtime becomes
the admin. Guest creation is disabled by default. When an admin enables it in
System, new people create their own guest passkey from Home; the admin controls
the enrollment policy but does not create or hold the guest's authenticator.
Every guest receives a separate principal and `localhost://Users/<principal-root>`
area. Disabling guest creation stops new guest areas from being created, but
existing guests can still sign in until their passkey or grants are explicitly
revoked.

Guest privacy must be real, not courtesy UI. Admins may operate the runtime,
revoke local access policy, and manage availability, but they should not be able
to decrypt a guest's personal root without that guest's explicit recovery,
sharing, legal/operator policy, or future threshold authorization path. Public
Home deployments should therefore be safe for self-created guest accounts that
can later export recovery material and migrate their encrypted root to their own
ElastOS runtime.

Passkey credentials are access proofs, not the data itself. Removing every
passkey should revoke proof bindings and sessions, while the corresponding
`localhost://Users/<principal-root>` data remains on disk as an orphaned root.
Normal UX should not auto-attach that root to a new passkey; recovery needs an
explicit reassignment flow with signed audit. System now supports the first
version of that flow: importing a verified Recovery Kit for an orphaned root can
rebind the active passkey to the recovered principal/root and reissue Home/System
session tokens. Future DID-backed reassignment should extend the same contract,
not bypass it.

The secure-at-rest target is principal-root encryption, not "passkey equals
encryption key." WebAuthn normally proves user presence/control without exposing
private key material to the site. ElastOS should create a random per-principal
data key, encrypt user-root objects through the runtime/provider plane, and wrap
that key only to explicit protectors such as WebAuthn PRF when available,
DID-backed recovery, an exported recovery phrase, or a user-held recovery kit. If
every protector is lost, encrypted data should be unrecoverable by design rather
than silently accessible through a device-global bypass.
WebAuthn PRF must be implemented as client-side data-key wrapping: raw PRF
results are key material and must not be sent to runtime auth routes. The
runtime stores protector metadata and wrapped envelopes, not PRF output.

Recovery and migration should use crypto-agile, quantum-conscious envelopes from
the start: AES-256 or ChaCha20-Poly1305 for bulk encryption, ML-KEM-768 or
stronger for future public-key wrapping, HQC as a later backup KEM when
standardized, ML-DSA plus optional SLH-DSA for durable signatures, and explicit
metadata for every algorithm. Current passkeys, EVM, BTC, ELA, and Ed25519 DIDs
remain classical proofs; long-lived protected data must not depend on those
alone as the permanent recovery root.

The first runtime slices are now a proof-bound recovery contract and protected
root-object enforcement. A principal can create and later download a Recovery
Kit through active Home/System authority, the runtime stores a principal-bound
encrypted archive plus verified protector metadata, downloads can be wrapped
into a password-protected Recovery Kit package, and imports must verify the
wrapped data key plus encrypted root descriptor before the root is marked
recoverable. `did-provider` now has the first typed `did:key` recovery-proof
verification primitive, and Recovery Kit import now consumes that proof through
`did-provider` when it matches an existing DID recovery protector on the
recovered root. This is still not DID-only recovery: the import path still needs
Recovery Kit material until DID-envelope unwrap/rewrap exists, and
`did:elastos`/EID verification still needs a resolver-backed adapter.
When a root has verified protection, Documents working copies, Home browser
state, and viewer/content storage are written through a runtime-owned
AES-256-GCM object envelope bound to principal, root, data-key ID, and object
URI. Protected roots reject plaintext reads instead of silently migrating.

Identity semantics should stay layered. A passkey unlocks a runtime principal.
The device/node uses a `did:key` device DID for Carrier/provider signing.
`did:elastos`/EID is the global account/credential/namespace path. CIDs identify
immutable content graphs, while IPLD connects those graphs into signed heads,
manifests, provenance, rights, and availability records. A local handle like
`alice` is only display copy unless it is resolved through a registry; a global
name claim needs EID/DID-chain or equivalent namespace consensus to prevent
double claims. Capability checks should never depend on an unverified handle.

IPLD belongs in this model as the content-addressed object graph and manifest
data model. It can make ElastOS objects, channel heads, provenance records, and
availability receipts traversable by CID. It is not the Carrier network, not a
pinning guarantee, and not the rights/decryption layer. The current architectural
direction is documented in [docs/CONTENT_AVAILABILITY.md](docs/CONTENT_AVAILABILITY.md).

This keeps the four ElastOS quadrants balanced. The canonical quadrant
definition lives in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#elastos-four-quadrants);
this roadmap only sets sequencing.

The near-term architecture should rebalance those quadrants in this order:

1. passkey-first Runtime authority and human/agent delegation
2. content availability and IPLD-compatible published-object manifests
3. Runtime-mediated protected content: sealed objects, rights checks, key release, and decrypt/render providers
4. wallet/DID/node proof adapters behind Runtime authority
5. Spaces/network drives
6. capsule publish/install registry

Those moves strengthen all four quadrants without making any one substrate the
root of trust. Rich DRM economics, literal Capsule-NFT mechanics, Android-box
specifics, and DeFi/BtcFi integrations come later, after principals, packages,
interfaces, availability receipts, and spaces are real.

PC2 is a useful implementation reference for this sequence, not a competing
authority model. Wallet bridge method classification, IPFS cluster/supernode
availability work, dDRM contracts, WASM decrypt/render helpers, and runtime
heartbeat patterns are convergence inputs only after they are translated into
Runtime principals, scoped capabilities, provider-owned effects, and signed
audit. The current translation is tracked in
[docs/PC2_CONVERGENCE.md](docs/PC2_CONVERGENCE.md).

The first source-reference migration slices should stay product-useful and boundary-small:

1. **Library / WebSpace**: browse, upload, download, open,
   publish, share, and inspect files/objects through Home/Library,
   principal-root storage, persisted WebSpace mount/object-head metadata,
   WebSpace lifecycle/health receipts,
   `elastos://content/*`, recipient share-grant records, recipient-scoped
   shared-access checks, and availability receipts with honest
   peer-selection/quota/repair-worker metadata.
   Preserve useful file-manager behavior where it helps users, but translate every
   operation onto typed Runtime object/provider contracts instead of older
   filesystem, Puter, or direct IPFS assumptions.
2. **AI Chat**: bring the chat UX over as a provider-backed app capsule where
   inference, hosted-model credentials, embeddings, and document context
   expansion stay inside `ai-provider`, `llama-provider`, or an operator-pinned
   hosted provider.
3. **dDRM + Elacity Marketplace foundation**: wire protected-content provider
   contracts before Marketplace/Creator/Player/Viewer UX. The sequence is
   content status/fetch, rights check, key release, decrypt/render session,
   receipt, Wallet/Inbox approval where needed, and audit.

Those slices are intentionally ordered so the user can first manage and publish
ordinary objects, then use provider-backed AI over those objects, then add
protected-content economics without giving apps raw keys, wallets, chain RPC,
Kubo/IPFS, Elacity SDKs, or provider credentials.

COMO is a separate runtime-framework research input, not a planned dependency.
Its C++ component model, runtime reflection, MetaClass packaging idea, Android
aarch64 history, and safety/redundancy lessons may inform the capsule-kernel ABI
and generated interface glue. ElastOS should still keep the trusted foundation
Rust/Wasm/WASI-first unless research proves a narrower, capability-preserving
integration path. Track that work in
[docs/RUNTIME_FRAMEWORK_RESEARCH.md](docs/RUNTIME_FRAMEWORK_RESEARCH.md).

Browser work must follow the same model. The Browser capsule is not the
platform and must not become an ambient host-web escape hatch. The stable product
contract is one Browser/Net/Exit ABI: the Browser asks Runtime for a scoped
session, Runtime grants explicit network and wallet capabilities, the engine
renders the web, and Exit/provider policy owns egress. Local launcher and device
builds should prefer a native Chromium/CEF-style adapter with real compositor,
input, audio, and OS-level network isolation. Hosted Home may use a remote
browser provider only if it is adapted behind the same ABI and passes the same
audio/video/input/wallet/direct-network/manual UX gates. Docker/Selkies is
operator packaging for the current hosted baseline, not the architecture and not
the acceptance answer. If a host cannot prove native audio/compositor/network
isolation and no hosted provider is provisioned, Browser work should stop at the
contract/gate layer instead of accumulating more proof-surface tuning.

### Planning Review Gate

Future plans should pass this gate before implementation:
- **First-principles fit:** the work strengthens local object identity, explicit capabilities, and no-ambient-internet capsule authority.
- **Smallest shippable slice:** the plan names one concrete runtime behavior or user journey that can be verified end to end.
- **Quadrant balance:** the plan states what it changes in PC2/Home, Runtime, Carrier, and Blockchain, including "nothing" where a quadrant is intentionally unaffected.
- **Boundary clarity:** app/viewer/content capsules remain protocol-agnostic; provider-specific behavior stays in provider/system-service code.
- **Proof path:** the plan names the command, smoke test, or manual loop that proves the change.
- **Entropy check:** the plan removes or avoids duplicate truth surfaces, stale alternate paths, stale names, and speculative hooks.

## Near-Term Direction

### 0. Enforce capsule authority through the runtime/Carrier plane

Normal app, viewer, and content capsules should be Carrier-only by default:
- no `guest_network`
- no host process execution
- no raw off-box transport
- no direct IPFS/file/database access
- no provider-specific protocol knowledge in app UI

Provider capsules and explicit system services are the narrow exception. They
may know about concrete protocols, but only behind manifests, capability schemas,
audit events, and user/operator-visible reason strings.

The gateway edge must stay thin. It authenticates browser/host adapters, checks
capabilities, and routes operations. It should not quietly become the provider
implementation for IPFS, social drives, wallet signing, or collaboration logic.

### 1. Build content availability as the default SmartWeb object plane

CID creation is not enough. Publishing should mean the object is packaged,
locally pinned, submitted to the configured SmartWeb availability network, and
tracked by signed availability receipts.

The first implementation should be deliberately layered:
- expose `elastos://content/*` as the capsule-facing product contract
- keep `elastos://ipfs/*` only as the current low-level system/provider backend around Kubo, then retire it from the normal capsule-facing namespace once `elastos://content/*` exists
- model published objects, signed heads, provenance, and availability receipts with IPLD-compatible JSON/CBOR shapes
- keep local-only availability receipts honest by carrying explicit
  peer-selection/quota/repair-worker metadata instead of implying live
  multi-peer replication
- use Elacity/supernodes as the first remote availability target
- add volunteer replication and repair loops behind provider policy
- add payment/storage incentives only after receipts, quotas, health checks, and abuse controls exist

Carrier belongs in this slice as the secure coordination and peer/object
transport substrate. The content provider owns availability policy. IPFS/Kubo
and cluster-like systems are replaceable block backends underneath.

### 2. Build Runtime-mediated protected content

Protected content should use the same no-ambient-internet model:

`viewer capsule -> runtime capability -> elastos://drm/open -> rights/key/decrypt providers`

The first slice is deliberately fail-closed: shared sealed-object schemas,
`drm-provider` status/open, `rights-provider` typed policy questions, and typed
`chain-provider` `has_access_by_content_id` reads that only call configured
contracts/selectors. That creates the boundary without pretending dDRM or dKMS
is production-ready.

The remaining order is:
- wire `elastos://drm/open` through content status/fetch, typed rights checks,
  key release, decrypt/render sessions, signed release receipts, and audit
- publish new protected content as encrypted sealed objects with rights policy,
  algorithm-agile key envelope, availability receipt, viewer interface, and
  provenance links
- add permissioned ElastOS PQ-hybrid dKMS v0 for new content only: AES-256 CEK,
  `t-of-n` shares, hybrid X25519 + ML-KEM share wrapping, and classical + PQ
  release receipts
Normal capsules still receive scoped output or a scoped decrypt session, not raw
CEKs, key-backend SDKs, wallet RPC, chain RPC, Kubo/IPFS APIs, or Elacity
credentials.
FROST can be a classical helper for v0 receipt/cohort signing, but it is not the
long-term dKMS root because Schnorr/ECC security is not post-quantum.

#### Secrets Vault / Password Manager Direction

dKMS and dDRM can also support a password manager, but they are the substrate,
not the product by themselves. The product layer should be a Runtime-mediated
Secrets Vault:
- store passwords, API keys, recovery codes, and other small secrets as typed
  encrypted objects under the active principal root
- use random per-secret data-encryption keys, wrapped only to explicit
  protectors such as the principal data key, WebAuthn PRF when available,
  DID/recovery protectors, or dKMS recipient grants
- use dDRM-style rights, grants, revocation, and audit when a secret is shared
  with another principal, device, agent, or runtime
- expose secrets to apps and Browser only through scoped Runtime capabilities;
  app capsules and web pages must never receive raw vault APIs or key material
- bind Browser autofill to verified origin, user approval, and isolated
  Runtime/provider insertion rather than page-visible JavaScript access
- harden local unlock, recovery, device revocation, clipboard timeout,
  plaintext logging prevention, phishing/origin warnings, and export/import
  before calling it a full password manager

This should come after the protected-root/key-envelope work is stable. The
first slice can be a Secrets capsule plus provider contract for create/read/
update/delete/generate/share/revoke, with Browser autofill added only after
origin binding and approval UX are proven.

### 3. Build passkey-first authority, then blockchain proof adapters

The authority foundation is not EID, Essentials, UniversalX, BTC, Base, SIWE, or
passkeys alone. The runtime foundation is:

`runtime principal -> verified proof bindings -> short-lived session -> scoped capability -> provider-mediated effect -> signed audit`

Passkeys/WebAuthn should be the default human proof binding for Home because
they are local, phishing-resistant, recoverable across devices, and do not force
wallet-first onboarding. Agents do not use passkeys directly; humans approve or
delegate scoped grants, and agents operate through those grants with the same
capability checks and audit trail. Blockchain-specific systems are adapters
behind the same authority model:
- EVM/SIWE proves control of an EVM account on Base, ESC, EID, or another EIP-155 chain.
- Essentials and Elastos Wallet JS SDK prove ELA mainchain and Elastos-native wallet authority.
- EID anchors higher-trust DID, credential, recovery, publisher, service-endpoint, and DAO identity.
- BTC proves Bitcoin address control and node-backed chain state.
- UniversalX/Universal Accounts may improve onboarding, balances, deposits, gas abstraction, and transaction UX, but must not mint runtime principals, sessions, or privileged capabilities directly.

Keep the runtime nouns separate:
- **Principal**: person, agent, device, capsule, or provider.
- **Proof binding**: EVM account, BTC address, `did:key`, `did:elastos`, Essentials/EID proof, or other verified subject.
- **Session**: ephemeral runtime grant context bound to principal, proof, device/browser, expiry, and scope.
- **Capability**: narrow authority for one capsule/provider/object action.
- **Audit event**: signed record of challenge, proof verification, grant, effect, revocation, or denial.

The first shippable authority slice is deliberately small:
- create a runtime WebAuthn challenge for Home/System
- verify a passkey proof against RP ID, origin, challenge, expiry, user verification, and signature counter
- bind each passkey proof to its own runtime principal and local user root
- issue scoped Home/System grants
- audit the grant and revocation
- fail closed for replay, expiry, wrong origin, wrong RP, missing user verification, counter regression, and missing grant
- enforce first-passkey-admin and guest-enrollment-off-by-default semantics

WebAuthn RP policy must stay explicit. Hosted Home
(`https://elastos.elacitylabs.com`) and local development (`localhost` /
loopback HTTP) are separate passkey worlds because WebAuthn credentials are
origin/RP scoped. A PWA inherits the origin it was installed from. Future mobile
or WebView adapters must present a stable secure origin or a native host-auth
adapter that the runtime models explicitly; they must not silently reuse hosted
or localhost passkeys through a header-based bypass.

The first wallet implementation reuses that authority shape in two paths:

- built-in managed wallet: provider-held encrypted keys for ESC/Base EVM and
  Bitcoin mainnet P2WPKH, created after passkey unlock, used only after
  Wallet/Inbox approval, and recorded as a signature receipt/audit event
- injected EVM wallet: MetaMask-compatible SIWE proof through a dedicated
  connector capsule, linked to the existing Runtime principal with a
  connector-bound account record, never used as the Home login root or hosted
  inside System
- injected Bitcoin wallet: UniSat BIP-322 simple proof through a dedicated
  connector capsule, linked to the existing Runtime principal with a
  connector-bound account record; the first proof class is Bitcoin mainnet
  native P2WPKH only until Taproot and other script vectors are pinned

The wallet proof adapter rules are:
- create a runtime SIWE challenge
- verify an EVM account proof against domain, URI, chain ID, nonce, issued-at,
  expiry, and resources
- verify ERC-1271 smart-account SIWE proofs through `chain-provider` before
  `wallet-provider` consumes the Runtime challenge
- verify Bitcoin BIP-322 simple P2WPKH proofs against exact Runtime challenges
  through the UniSat connector, and sign built-in Bitcoin
  proofs only from Runtime-bound approval payloads
- require an existing passkey-backed runtime session
- bind the wallet as a proof binding on the existing runtime principal
- bind the connector id to the linked account and any external signing approval
- issue only the wallet/chain capabilities approved by the user
- fail closed for replay, expiry, wrong origin, wrong chain, wrong resource,
  wrong connector, and missing grant

The wallet-provider contract is documented in
[docs/WALLET_PROVIDER.md](docs/WALLET_PROVIDER.md). It should be implemented
behind `elastos://wallet/*`, not as app-visible wallet RPC or a wallet-first
identity root. Browser wallet integrations must be dedicated connector
capsules. WalletConnect belongs here as a connector surface under
`wallet-provider` authority, not as app-level WalletConnect sessions and not as
raw SDK state inside ordinary capsules. Mode A comes first: ElastOS acts as the
dApp, `wallet-walletconnect` opens the operator-pinned Reown/AppKit adapter,
and `wallet-provider` records the verified proof, linked account, approval,
receipt, and audit. Mode B comes later: ElastOS acts as a WalletConnect wallet
for external dApps, with every request routed through Runtime approval and
signed audit. Internal capsules do not use WalletConnect at all; they call
`elastos://wallet/*`.

Public Wallet language should stay above connector details. Users see Wallet,
Accounts, balances, assets, activity, and approval methods. Provider code may
use signer, connector ID, proof binding, and chain namespace, but visible UI
should not split the product into "MetaMask wallet", "Bitcoin wallet",
"built-in wallet", and "WalletConnect wallet" unless the user explicitly opens
that approval method.

### Browser Capsule Direction

The browser must be a real browser capsule, not an iframe that happens to open a
website. The current `browser` capsule is a Runtime Browser proof shell only. The
target architecture is defined in [docs/BROWSER_CAPSULE.md](docs/BROWSER_CAPSULE.md):

`Browser UI capsule -> Runtime Browser open route -> Runtime Net/Exit providers -> Browser Engine Adapter -> selected network stream`

The first production implementation target should be a native/webview or
microVM browser engine adapter, not a full browser engine compiled to WASM.
Linux and Jetson should prove the model first with CEF/Chromium or
Chromium-in-microVM, direct outbound network denied at the host boundary, and
only Runtime Net proxy/IPC/vsock available to the engine. Windows, macOS, and
Android then implement the same Browser/Net/Exit ABI with platform-appropriate
adapters such as WebView2, CEF, WKWebView for constrained cases, Android WebView,
or GeckoView.

The important invariant is not that every local byte literally traverses a
remote Carrier hop. The invariant is that the browser has no ambient off-box
network. Same-machine adapter plumbing can use IPC or loopback below the capsule
contract, but all off-box effects are authorized by the runtime and routed
through `elastos://net/*` to Carrier/Exit providers. Browser dapp wallet access
must be a Runtime-mediated EIP-1193/WalletConnect bridge backed by
`elastos://wallet/*`, with requests surfaced in Wallet/Inbox before any
signing effect.

The first concrete Browser/Net provider is intentionally fail-closed:
`net-provider` validates Browser requests, blocks LAN/private targets by
default, and refuses direct host networking with an explicit `exit_unavailable`
handoff instead of touching host networking itself.
The matching `exit-provider` is also fail-closed today: it defines the internal
egress contract (`quote`, `open_stream`, `close_stream`, `http_fetch`) and
refuses egress until a backend is explicitly configured.
The first constrained backend is `http_fetch`, gated by
`ELASTOS_EXIT_PROVIDER_CONFIG`, host policies, body limits, and private-target
blocking by default. Host policy may be a narrow dapp allowlist or `"*"` for
public-web browser use, and it now includes explicit scheme/protocol and port
allowlists for the exit path. Private/LAN targets remain blocked unless
explicitly granted. It is a diagnostic/compatibility proof, not the final
general browser path. The visible Browser capsule calls `/api/apps/browser/open`;
the Runtime does the internal Net-validation-to-Exit-to-Engine handoff so apps
never receive `elastos://exit/*` or `elastos://browser-engine/*` authority.
The Browser open route now validates each internal provider handoff as a
resource-shaped call: `elastos://net/stream`, `elastos://exit/open_stream`, then
`elastos://browser-engine/launch`. `stream_relay` backends may now reserve typed stream-session receipts, and the
Browser Engine Adapter owns the actual byte/render surface so the Browser UI
never becomes a host iframe or host tab. HTTP-fetch is only a constrained
diagnostic/compatibility operation.
`browser-local-exit` is the first server-side Exit relay implementation. It
requires `ELASTOS_BROWSER_LOCAL_EXIT_CONFIG`, accepts typed
`elastos.exit.relay-open/v1` handshakes from Runtime, dials only
operator-approved public hosts/schemes/ports, and blocks private resolved IPs
unless explicitly enabled.
The first `browser-engine-adapter` provider now defines the internal adapter
contract and fails closed unless an operator configures an adapter and Runtime
passes a stream-session receipt with attached `adapter_ipc` byte transport.
`adapter_ipc` uses an internal `elastos.adapter-ipc/v1` descriptor that Runtime
passes to the Browser Engine Adapter and strips from Browser UI responses.
Native adapter kinds now launch only through an operator-approved supervisor
command. Runtime sends `elastos.browser.engine.launch-request/v1` through
`ELASTOS_BROWSER_ENGINE_REQUEST`, and the supervisor must return
`elastos.browser.engine.supervisor-result/v1` with runtime-net-only,
no-direct-network, and no-wallet-injection proofs. The Rust Linux helper
`browser-engine-supervisor` reads `ELASTOS_BROWSER_ENGINE_SUPERVISOR_CONFIG`,
launches the configured engine under `linux_new_netns`, and emits that typed
proof. The server/headless proof uses
`elastos/tools/browser-playwright-engine` to launch Playwright Chromium through
the same adapter contract, render public HTTPS pages through the configured
server Exit, and return a Runtime diagnostic frame/input control surface.
The first bridge helper is now `browser-stream-bridge`: it accepts the private
engine-side Unix socket and forwards bytes only to a Runtime-owned Unix stream
socket. It has no TCP, DNS, HTTP, wallet, chain, or raw host-network path. The
supervisor can launch it before the native engine when operator config declares
a `stream_bridge` program and the internal descriptor carries
`runtime_stream_path`. Gateway now allocates that private path under the short
host temp directory `elastos-browser-streams/` before Browser Engine launch and
strips the full descriptor from Browser UI responses. The short path avoids Unix
domain socket length failures while keeping the descriptor private Runtime
plumbing. Gateway binds the private socket as a one-shot listener and relays
bytes only when the Exit provider returns a private
`elastos.exit.relay-ipc/v1` Unix socket; otherwise it accepts and closes
fail-closed. Browser UI never receives `adapter_ipc` or `relay_ipc`; internal
Browser Engine Adapter/Supervisor calls may receive `relay_ipc` only to open
Runtime-mediated Exit streams without host TCP authority.

The first native wrapper is `browser-native-proxy-engine`. It runs inside the
supervised native engine sandbox, starts a loopback HTTP proxy for Chromium/CEF,
receives `ELASTOS_BROWSER_ENGINE_RELAY_IPC`, and turns browser `CONNECT` or
absolute-form HTTP proxy requests into typed `elastos.exit.relay-open/v1`
handshakes against the Runtime Exit relay Unix socket. This keeps browser
semantics in Chromium/CEF while preserving the ElastOS rule that off-box traffic
uses Runtime/Carrier-shaped provider contracts. The local smoke
`scripts/browser-native-proxy-engine-smoke.sh` proves the wrapper can run an
actual child process through that proxy and relay path. The host-gated smoke
`scripts/browser-native-supervisor-proxy-smoke.sh` proves the same wrapper runs
under `browser-engine-supervisor` with direct TCP/DNS blocked inside
`linux_new_netns` while Runtime Exit relay IPC still works.
`scripts/browser-native-operator-config.mjs` now generates the matching
`browser-engine-adapter.json`, `exit-provider.json`, and
`browser-local-exit.json` files for a target host so the native Browser path has
one deployable config source instead of hand-written nested JSON.
`scripts/browser-native-target-preflight.sh` wraps that config generation with
actual provider initialization checks and fails closed when the host-gated
namespace/proxy proof skips, so native Linux/Jetson support is not marked proven
on a host that cannot isolate browser networking. Product media readiness is
stricter: `--native-audio --native-video --require-native-media` must produce a
typed preflight receipt with both `native_audio_proven=true` and
`native_video_proven=true`, not merely a config declaration.
`scripts/browser-native-host-capability.mjs` is the quick target probe before
that full preflight: it checks for a compatible browser binary, host
display/compositor, host audio service, and Linux network namespace support
without installing software or using Docker.

The diagnostic Playwright proof can render public HTTPS pages, including
`glidefinance.io` and exit-IP diagnostics, through the configured server Exit
and inject a constrained Runtime-mediated EIP-1193 provider for account/chain
discovery. It now round-trips actual URL/title after launch and input, resizes
to the Home window viewport, uses a long-polled frame route, and supports
wheel/click/paste/basic keyboard input. It is image-frame/input transport, not a
normal browser surface; it cannot be the final UX for video, audio, native
scrolling, text selection, or production dapp interaction. Browser
`personal_sign`, `eth_sign`, and EIP-712 typed-data signing now create typed
Wallet/Inbox approval requests bound to principal, session, account, chain, and
page URL; the page promise resolves only after Wallet/Inbox approval and
managed or connector signature completion. Managed `eth_sendTransaction` now
follows the same
authority shape: Gateway validates the page/account/chain request,
`chain-provider` prepares a typed transaction intent, Wallet/Inbox approves and
signs it, `chain-provider` broadcasts it, and the page receives only the
transaction hash. The provider call remains `elastos://wallet/*`, while the
approval resource is the actual chain effect (`elastos://chain/*/broadcast_transaction`).
External EVM accounts now use connector handoff: MetaMask and WalletConnect
approval capsules receive the typed transaction after Wallet/Inbox approval and
complete the request with the external wallet's transaction hash. A connector
smoke loads both connector capsules with fake Runtime endpoints and fake
EIP-1193 providers to prove known-chain add/switch handling, external
`eth_sendTransaction`, and transaction-hash-only Runtime completion.
The remaining connector work is live dapp proof, chain-switch ergonomics, and
clear fail-closed error copy. The product Browser path
is explicit display sessions with no fallback:
hosted Home uses `webrtc_remote_display`, launcher/mobile hosts use
`native_surface`, and diagnostic frames are never a downgrade path. Browser
Engine Adapter configs must declare supported `display_modes`; omitted modes are
unavailable by design. The hosted proof now has a Runtime-scoped WebRTC
offer/answer path, separate Runtime-scoped ICE candidate messages, and a
Playwright/CDP proof video sender. Playwright stays diagnostic/test
infrastructure; the product adapter should be native/compositor-backed Chromium,
CEF, or a microVM browser engine behind the same display-session contract. It
must advertise only implemented media capabilities, so hosted WebRTC remains
`audio: false` until real audio capture exists. Browser UI now ties muting to
the display-session audio flag, and Browser Engine Adapter rejects any
`proof_surface` session that claims audio; audio belongs only to a real
`product_compositor` backend. For hosted Home, Selkies/GStreamer-style
compositor WebRTC is the current self-hosted baseline, not the final product
default. It proves the Browser ABI, Runtime Exit routing, audio/video product
compositor receipt shape, and datachannel input, but repeated UX testing still
shows latency/quality limits and unresolved YouTube stress. The product strategy
is therefore native/local browser adapters for lowest-latency
launcher/mobile/Jetson surfaces, plus a bounded hosted-provider bake-off for
pure web deployments. Kasm Workspaces/KasmVNC and BrowserBox candidates must
use the same `hosted_remote_browser` contract and pass the same
audio/video/input/wallet/direct-network gates before replacing Selkies. The
operator config generator has named candidate presets (`selkies`,
`kasm-workspaces`, `browserbox`, `kasmvnc`) so the bake-off path does not
depend on hand-matched engine/backend strings. The
operator config and strict hosted product supervisor bridge now exist: the
bridge accepts only a Unix control service that returns a real
`product_compositor` display session with `audio=true` and `video=true`. It does
not synthesize media or fall back to the Playwright proof.
The target-host preflight generates that config and runs the product display
gate against the real control socket before hosted Browser support can be
advertised. The Browser display contract now supports both browser-offer and
engine-offer negotiation; Selkies/GStreamer uses engine-offer, so a product
session must include `offerer=engine` and `initial_offer`, and Browser sends
the `elastos.browser.webrtc-answer/v1` back through Runtime signaling. The
first Selkies control bridge now exists in
`scripts/browser-selkies-control-service.mjs`: it translates the ElastOS hosted
product control socket into Selkies' `HELLO client` / `SESSION server`
signaling flow, requires private loopback CDP control so opens navigate an
actual browser page, and is covered by
`scripts/browser-selkies-control-service-smoke.sh`. A target preflight,
`scripts/browser-selkies-target-preflight.sh`, now starts that bridge against an
already-running Selkies WebSocket endpoint plus private CDP endpoint and runs
the hosted product-display gate through `browser-engine-adapter`. Authenticated
Selkies signaling is supported by the gate through explicit Basic auth
arguments, so product hosts do not need to weaken their control-plane auth just
to satisfy the preflight. A heavier repeatable Docker gate,
`scripts/browser-selkies-current-wheel-smoke.sh`, now validates the current
Selkies Python wheel with Xvfb/PipeWire/PipeWire-Pulse against that authenticated
preflight. `scripts/browser-selkies-real-chromium-smoke.sh` now extends that to a
real Chromium/CDP page on the Selkies display while preserving
`product_compositor` audio/video and no raw DNS.
`scripts/browser-selkies-runtime-exit-smoke.sh` is the current best self-hosted
baseline proof: it runs `browser-local-exit`, launches Chromium inside the
Selkies target through `browser-native-proxy-engine`, verifies a real page load
through Runtime Exit, and passes the authenticated Selkies product-compositor
audio/video gate. It is not enough to call the Browser complete. The hosted
provider comparison gate now also includes
`scripts/browser-hosted-product-navigation-smoke.sh`, so address navigation,
back, forward, and reload must work through the Runtime/provider input route
with `direct_network=false` before a hosted browser provider can be treated as a
replacement candidate. `scripts/browser-selkies-runtime-exit-target.sh` is now the
matching operator launcher and writes the Runtime `browser-engine-adapter.json`,
so the smoke and operator path share one start sequence. The controlled
operator-image and durable service wrapper path now exists; the remaining
hosted work is YouTube/operator Exit hardening, real TURN/operator
configuration where needed, subjective UX/performance tuning,
per-user/per-page hosted session isolation, and running non-Selkies candidates
through the same bake-off before replacing the baseline.
The native Linux
supervisor now returns an explicit `native_surface` display session for
launcher/mobile-style product hosts, and `browser-native-proxy-engine` is the
first native Chromium/CEF wrapper that keeps networking behind Runtime Exit IPC.
Native media capability is no longer assumed: the supervisor only reports
native audio/video from explicit operator `display_capabilities`, and the fake
namespace/proxy smokes keep both false so they cannot be mistaken for an
audio/compositor proof.
The Browser media stress input is `scripts/browser-youtube-acceptance-smoke.sh`:
it must prove YouTube playback with decoded video/audio bytes through the
Runtime proxy path before a candidate can be considered media-capable.
`browser-local-exit` now has explicit `address_family` policy and prefers IPv4
by default because YouTube treats this server's IPv6 route as captcha/sorry
traffic. That route fix preserves the Runtime/Exit contract, but a narrow
fixture result is not product audio acceptance by itself. Several other YouTube
URLs still trigger upstream bot challenges on this server Exit, so broader
YouTube coverage requires an approved Exit/provider route or trusted operator
profile, not a Playwright-only workaround. `browser-local-exit` supports an
operator-approved upstream HTTP CONNECT Exit for this purpose; credentials stay
in operator config and capsules still see only Runtime-mediated Browser/Exit
receipts.
`scripts/browser-native-supervisor-smoke.sh` is the host-gated proof for
`linux_new_netns`: direct TCP, DNS, and HTTP must
fail inside the engine process while Runtime Unix stream-bridge traffic still
works. `scripts/browser-native-supervisor-proxy-smoke.sh` extends that proof to
the native proxy wrapper path. The remaining browser proof work is native
CEF/Chromium or microVM display for Linux/Jetson, running that proof on a target
host, and the Glide wallet signing flow through Runtime. The Browser UI and Gateway already have
the Runtime-scoped WebRTC offer/answer/candidate signaling contract; it stays
fail-closed unless an engine declares and returns a real
`webrtc_remote_display` session.

The first Wallet surface should remain narrow: native account balances through
typed chain-provider reads, fiat/native valuation through the runtime-owned
`/api/wallet/prices` price service, account/default selection through
wallet-provider, receive QR generation through the Wallet gateway contract, and
approval-method flows for MetaMask, UniSat external Bitcoin proofs, and
WalletConnect. Token/NFT asset reads, richer activity history, forecasting, and
fully wired send signers come later as provider-backed views, not direct app RPC
integrations.

Price data is authority-sensitive even when it is "read-only." The durable path
is a typed price/oracle provider backed by chain oracles and signed provider
receipts. External HTTP price sources are allowed only when the operator
explicitly configures the source and approval flag; ordinary capsules never call
CoinGecko, exchange APIs, browser fetch, or raw web endpoints for balances or
pricing.

Do not add a visible WalletConnect capsule backed by an unpinned CDN, missing
project configuration, or a bundled public Project ID in the repository. The
runtime-side connector gate should accept `wallet-walletconnect` only when an
operator has pinned both connector config and a local SDK asset hash. Official
deployments and independent operators must pin their own Reown Project ID plus
local adapter hash in runtime config. The source capsule may exist before that,
but it must stay hidden/unroutable until the pinned Reown/AppKit adapter proof
exists.

The first node-access slice is also deliberately small:
- expose `chain-provider` as `elastos://chain/*`
- default to production-only chain reads: ELA mainchain typed REST, ESC EVM JSON-RPC, Base EVM JSON-RPC, and BTC typed REST status
- add typed status/sync proofs, unsigned transaction preparation, signed transaction broadcast, persistent lifecycle status, and operator-approved local lifecycle control without exposing raw node ports
- keep testnet and identity-chain networks out of the default System surface until they have a concrete wallet/DID journey
- allow operator-owned Bitcoin Core as an explicit loopback override, not a hosted default
- keep backend RPC URLs and node ports hidden from capsules
- allow managed local node start/stop/restart only for explicit loopback supervisor config, without changing the capsule-facing contract

Hosted development should use the provider proxy path first. Heavy local node
daemons belong behind explicit operator policy and host-resource review, not in
the default Home setup.

Keep blockchain UI limited to passkey login, Wallet-owned account/linking and
approval flows, and System diagnostics until provider manifests, capability
schema, lifecycle policy, and verification commands cover node lifecycle and
write/broadcast operations. Node providers are infrastructure/provider capsules
with persistent attached state and explicit network authority, not ordinary apps.

### 4. Keep Home as the runtime-owned browser-host adapter

- keep `/apps/home/` as the runtime-owned browser-hosted adapter for the Home capsule
- keep `home` as the internal capsule ID for the visible Home surface
- keep `home-cli` as the installed WASM CLI capsule for the terminal front door
- keep the current Home contract real: identity summary, app/object catalog, validated launch routes, and app-scoped launch tokens
- grow System from that first slice into real runtime/object management
- prove one truthful `Home -> System -> app/object -> Home` loop
- keep install-profile and release integration on the same `home` / `home-cli` names
- do not reintroduce donor or VM-only Home lanes into the Home contract

The important constraint is architectural ownership:
- the runtime owns identity, capability, object access, hosted-route policy, and capsule lifecycle
- Home is a capsule consuming those contracts
- System is another app launched by Home through runtime contracts
- the default Home path must stay compatible with macOS, so it cannot depend on Linux/KVM-only behavior

### 5. One runtime contract for executable capsules

Converge native, WASM, and microVM capsules on one explicit contract for:
- identity bootstrap
- capability acquisition
- Carrier access
- localhost storage access
- interactive TTY ownership
- home/exit signaling

Do not keep multiple half-compatible runtime stories alive.

### 6. Make Home a boring front door

Home should stay inside one owned interactive session and make the main user path obvious:
- launch
- navigate
- open a surface
- return home cleanly

The runtime should support that without CLI detours, TTY confusion, or host-specific guesswork.

`home` is not a shortcut around this requirement.
The current Home surface must keep the same boring front-door properties rather than hiding them behind a prettier UI.

### 7. Keep release, install, and update on one truthful path

The product path should remain:
- signed install
- trusted source configuration
- plain `elastos update`
- fail-closed behavior when trust is missing

Operator/debug paths can exist, but they must stay explicit and secondary.

### 8. Keep the rooted object model coherent

The runtime should keep strengthening the relationship between:
- `localhost://...`
- `elastos://...`
- WebSpace-style mounted views

The goal is one stable object model, not a pile of one-off path conventions.

The object model should lead with human concepts, not implementation seams.
Users should primarily think in terms like people, spaces, sites, shares, apps, and agents rather than providers, runtimes, gateways, or transport details.

The same applies to off-box content.
Sharing, opening, public links, and site publication should read as operations on the same runtime objects, not as separate products with different transport stories.
Carrier should be the secure plane that carries those off-box interactions from the user's point of view, while lower-level storage or distribution mechanisms remain implementation details.

One concept should survive across multiple realizations.
If something appears in Home, under `localhost://...`, as an `elastos://...` object, or through a public URL, it should still read as the same underlying thing rather than four different products glued together.

Keep the ontology small and flexible.
Prefer a minimal set of durable concepts and role-based views over a deep rigid hierarchy that will be wrong once the system grows.

### 9. Build native collaboration and content around Carrier and runtime objects

Native `Chat` is the proving surface.
IRC and other compatibility surfaces may help earn the runtime contract, but they should not replace the target architecture.
Long term, communication and content exchange should be Carrier-first, capability-gated, and built on runtime/provider boundaries rather than classic centralized web-server assumptions.

Carrier should not appear to users as "just chat transport."
It should become the trusted off-box plane for:
- presence and direct communication
- signed content exchange and discovery
- object publication and retrieval
- site and share promotion across machines

The user contract should stay simple:
- chat, share, open, and site flows operate on runtime objects
- security and identity are consistent across those flows
- transport/storage internals do not leak into the primary product story

### 10. Keep site and publication flows local-first

`MyWebSite`, publication, release channels, and public serving should keep moving toward one coherent local-first story with explicit promotion and rollback.
The runtime should own the object/state model cleanly, even when gateways or public edges sit in front of it.

Over time, the same Carrier-secured plane used for collaboration should also make content and publication feel like part of one coherent system rather than a collection of unrelated subcommands.

## Later Direction

### Cross-platform runtime and host adapters

The long-term shape is one ElastOS contract above multiple host adapters. The runtime, the capability model, the namespace, and the capsule contract are the same everywhere. What changes is how the host presents capsules to the user.

**Host adapter modes:**
- **Server / headless:** Runtime serves capsule UIs over HTTP. Home is a web dashboard accessed from any browser. No local GPU or window manager required. This is the home server, NAS, or cloud deployment model.
- **Desktop (Linux, Windows, macOS):** Runtime opens capsule UIs in browser tabs or native windows. Home is the local launcher. GPU is available for rendering. Capsules that produce web UI open in the browser; terminal capsules open in terminal windows.
- **Mobile (Android, future iOS):** Runtime is a background service. The launcher is a native app. Capsules render in embedded webviews. The capability model gates sensor, storage, and network access the same way it does on desktop.
- **Kiosk / dedicated device:** Runtime owns the full display. Home is the desktop environment. Capsules launch fullscreen or in managed windows. This is the Jetson, set-top box, or dedicated appliance model.

**Capsules don't know which host adapter they're on.** A capsule that serves HTML on its HTTP port works identically on a headless server (proxied through the runtime), a desktop (opened in the browser), or a mobile device (rendered in a webview). The Carrier bridge, provider access, and capability model are identical regardless of host.

Linux remains the truthful full-runtime baseline. Other platforms should be earned without pretending to offer Linux/KVM parity everywhere. The default Home path should therefore be the browser-hosted path above the runtime contract, not a KVM-dependent appliance path. That keeps macOS, Windows, remote browser, and later mobile/webview adapters in scope without weakening the trusted-core model.

### Native object model and content-first design

The compatibility path (packaging existing web apps as capsules) gets existing software into ElastOS. But the native app model should be designed from first principles around **objects, not applications**.

**Core idea: everything is a typed object in the namespace.**
A photo is not `~/Photos/IMG_001.jpg`. It is `localhost://Users/<principal-root>/Photos/IMG_001` — a typed object with metadata, preview capability, provenance, and access control. The runtime knows it is an image. Home can render a preview without launching a capsule. A capsule requests access to `localhost://Users/<principal-root>/Photos/*` and gets typed objects back, not raw bytes.

**Apps don't own content, they view it.**
The `viewer` field in capsule.json already points this direction — gba-ucity is a data capsule, gba-emulator is its viewer. Scale that up: a PDF is a data object, a PDF viewer capsule renders it. An image is a data object, a gallery capsule renders it. The runtime resolves which viewer handles which type. Users open objects, not apps. The runtime picks the viewer.

That requires keeping three axes explicit in the runtime contract:
- execution substrate (`wasm`, `microvm`, `oci`, `data`, ...)
- product role (`shell`, `app`, `viewer`, `provider`, `content`)
- launch exposure / orchestration rights (Home-only, gateway-only, shared)

Do not keep inferring product meaning from one overloaded manifest field.

**Home is the object browser.**
Home evolves from "launch apps" to "navigate your objects." The natural tabs become:
- **Home** — recent objects, pinned spaces, activity stream
- **People** — identity objects (DIDs), conversations, shared spaces
- **Spaces** — rooted namespaces (Users, Public, MyWebSite, WebSpaces)
- **Apps** — installed capsule viewers and tools
- **System** — services, updates, trust configuration

Users navigate objects. Capsules appear when an object needs one.

People is also the right product surface for discovering trusted service offers,
but it must not become the provider control plane. A person/contact may expose
`elastos.service.offer/v1` cards such as conversation, remote Exit, storage,
relay, or model service offers. Enabling one creates or selects a
principal-scoped provider grant; the provider still enforces policy, quotas,
expiry, and audit. Browser, storage, or AI capsules then discover enabled
services through their provider contracts, not by reading People state directly.
Home carries the same records in a top-level `elastos.runtime.services/v1`
summary: local offers are what this Runtime can advertise, remote offers are
trusted People/Carrier discoveries, and capsules still see only capability
contracts such as `capsule -> runtime capability -> provider grant -> service`.
Current local offers are conversation hosting, Browser Exit, Browser Engine,
object storage, content availability/pinning backed by the content/IPFS
provider plane, and webspace hosting when their provider config or binary is
installed. Browser Engine offers carry a runtime-contract summary so a gateway
can distinguish a local microVM substrate from a remote/operator VM substrate
without inventing a non-VM Browser provider. Vault/password-manager style UX
belongs on top of an explicit future
vault/content/key provider contract; it should not be advertised as a current
service or as raw IPFS access.

**The browser is a capsule, not the platform.**
A web browser capsule gets `localhost://Users/<principal-root>/Bookmarks/*` and explicit outbound network capability. It is one viewer among many, not the runtime itself. This is the inversion from ChromeOS: instead of everything running in the browser, everything runs in the runtime and the browser is one sandboxed capsule.

The correct implementation target is the Browser/Net/Exit ABI, not a specific
engine. A browser capsule should request `elastos://net/resolve`,
`elastos://net/connect`, `elastos://net/stream`, and only constrained
`elastos://net/http` effects through Runtime capabilities. General browsing
should prefer stream relay where the browser engine owns TLS; an exit provider
forwards TCP/QUIC streams and does not become a trusted HTTPS MITM. HTTP-fetch
proxying can exist later for caching, controlled content, or compatibility, but
it must be a distinct capability and policy choice.

Exit providers are provider capsules or system services: local Runtime exit,
remote Carrier-routed exit, privacy/Tor-like exit, paid CDN/privacy exit, or
enterprise-policy exit. Site rules, profile selection, private-network blocking,
uploads/downloads, and metadata audit belong in the Runtime/provider contract.
The browser engine can start as a native/webview or microVM engine with no
ambient NIC except the Runtime bridge. A full WASM/WASI browser engine remains
R&D behind the same ABI, not the first product dependency.

The first `browser` capsule now exists as a visible Runtime Browser proof. It
opens inside Home, requests `/api/apps/browser/open`, and can render
public HTTPS pages through the Browser Engine Adapter when the operator
configures a public-web Exit policy. The current hosted Selkies/GStreamer path
is a baseline WebRTC product-compositor proof, while the older Playwright/CDP
frame path is diagnostic only and must never become a silent fallback. That
proof is not the final browser UX or security boundary. HTTPS dapps do not
receive signing authority until Browser/Net/Exit and wallet request approval
mediate the request through Runtime authority. `/apps/*` remains a host adapter
and edge transport surface; it must not become the source of truth for launch
rights, object identity, or capsule role.

`chat-room` is the concrete example. It should exist once as one capsule identity. Inside ElastOS, Home launches `chat-room` through runtime orchestration rights, opens its web surface in a managed window, and that surface keeps using Home-scoped authority instead of browser cookies. Outside ElastOS, the gateway serves the same `chat-room` surface to a normal browser under browser-session capability policy. The surface adapter is shared; the authority model is not.

Browser access review should also stay generic:
- the browser session is the remote principal
- `room.access` is a capability granted to that principal
- pending browser-access requests surface in Inbox as reviewable actions
- approving in Home authorizes that browser session without leaking Home rights to it

**The marketplace is a WebSpace.**
`localhost://WebSpaces/Marketplace` resolves to a typed catalog of published capsules with signatures, descriptions, versions, and install actions. Installing from the marketplace is `elastos capsule install <name>`. The marketplace capsule provides the UI; the runtime provides the trust verification and signature checking.

**Digital assets are typed by the namespace.**
The resolver layer knows:
- `localhost://Users/<principal-root>/Photos/*` → image objects
- `localhost://Users/<principal-root>/Music/*` → audio objects
- `localhost://ElastOS/Documents/<doc-did>` → mutable document objects
- `localhost://Users/<principal-root>/Documents/*` → working-copy storage for local document bytes
- `localhost://Users/<principal-root>/Models/*` → 3D model objects
- `localhost://Users/<principal-root>/Videos/*` → video objects

Each type has a default viewer capsule. The runtime dispatches. Home renders inline previews where possible. The same object model works across server, desktop, mobile, and kiosk — the host adapter decides how to present it.

**What needs to be built:**
- Typed object metadata in the namespace layer (localhost-provider returns type, size, preview — not just bytes)
- Viewer resolution (runtime maps object types to installed viewer capsules)
- Home as object browser (Home shows objects, not just launch buttons)
- Marketplace WebSpace (browsable catalog with install actions)

### Identity evolution

Keep `did:key` as the device/node foundation, not the human account root. Local
accounts are runtime principals unlocked by passkeys. Extend toward richer
profile coherence, persona separation, `did:elastos`/EID linking, and
chain-backed global names only when the local principal and recovery contract is
clean.

### Protected content and stronger attestation

Encrypted capsules, remote trust, reproducible builds, TPM/TEE-backed attestation, and dDRM-like flows remain future work.
They matter, but they should not distort the core runtime contract before the local base is stable.

### AI and operator surfaces

Agent and AI provider surfaces should keep moving toward one stable runtime contract with explicit policy, identity, and budget boundaries instead of ad hoc special cases.

## How to use this file

If a statement is a current proof claim, a release note, a version-specific fact, or a machine-specific result, it does not belong here.
This file should stay useful even when the next week of implementation details changes.
