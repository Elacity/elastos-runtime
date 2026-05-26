# ElastOS Runtime Roadmap

This file is for direction only.

For active work, see [TASKS.md](TASKS.md).
For current state, see [state.md](state.md).

---

## Mission

Make this repository the trusted local runtime layer of ElastOS:
- execute capsules predictably
- expose one coherent local object model
- use Carrier as the secure off-box plane for communication and content
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

This keeps the four ElastOS quadrants balanced. The canonical quadrant
definition lives in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#elastos-four-quadrants);
this roadmap only sets sequencing.

The near-term architecture should rebalance those quadrants in this order:

1. wallet-backed identity and WebConnect
2. Spaces/network drives
3. capsule publish/install registry

Those three moves strengthen all four quadrants at once. Rich DRM economics,
literal Capsule-NFT mechanics, Android-box specifics, and DeFi/BtcFi integrations
come later, after principals, packages, interfaces, and spaces are real.

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

Browser follows the same rule. The contract/gate layer is one Browser/Net/Exit
ABI: Browser UI asks Runtime, Runtime talks to Browser Engine Adapter + Net +
Exit providers, and engine/provider candidates do not get their own Browser
protocols. Selkies is useful as the current self-hosted baseline, but it is
not the final product and not the acceptance answer. Browser work should stop when
the structured provider `next_action` says to move to an operator-owned hosted
bake-off or native/local browser adapters for lowest-latency, real audio/video,
and a cleaner product path.

### 1. Keep Home as the runtime-owned browser-host adapter

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

### 2. One runtime contract for executable capsules

Converge native, WASM, and microVM capsules on one explicit contract for:
- identity bootstrap
- capability acquisition
- Carrier access
- localhost storage access
- interactive TTY ownership
- home/exit signaling

Do not keep multiple half-compatible runtime stories alive.

### 3. Make Home a boring front door

Home should stay inside one owned interactive session and make the main user path obvious:
- launch
- navigate
- open a surface
- return home cleanly

The runtime should support that without CLI detours, TTY confusion, or host-specific guesswork.

`home` is not a shortcut around this requirement.
The current Home surface must keep the same boring front-door properties rather than hiding them behind a prettier UI.

### 4. Keep release, install, and update on one truthful path

The product path should remain:
- signed install
- trusted source configuration
- plain `elastos update`
- fail-closed behavior when trust is missing

Operator/debug paths can exist, but they must stay explicit and secondary.

### 5. Keep the rooted object model coherent

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

### 6. Build native collaboration and content around Carrier and runtime objects

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

### 7. Keep site and publication flows local-first

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
A photo is not `~/Photos/IMG_001.jpg`. It is `localhost://Users/self/Photos/IMG_001` — a typed object with metadata, preview capability, provenance, and access control. The runtime knows it is an image. Home can render a preview without launching a capsule. A capsule requests access to `localhost://Users/self/Photos/*` and gets typed objects back, not raw bytes.

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

**The browser is a capsule, not the platform.**
A web browser capsule gets `localhost://Users/self/Bookmarks/*` and explicit outbound network capability. It is one viewer among many, not the runtime itself. This is the inversion from ChromeOS: instead of everything running in the browser, everything runs in the runtime and the browser is one sandboxed capsule.

Until that dedicated browser capsule exists, `/apps/*` remains a host adapter and edge transport surface. It must not become the source of truth for launch rights, object identity, or capsule role.

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
- `localhost://Users/self/Photos/*` → image objects
- `localhost://Users/self/Music/*` → audio objects
- `localhost://ElastOS/Documents/<doc-did>` → mutable document objects
- `localhost://Users/self/Documents/*` → working-copy storage for local document bytes
- `localhost://Users/self/Models/*` → 3D model objects
- `localhost://Users/self/Videos/*` → video objects

Each type has a default viewer capsule. The runtime dispatches. Home renders inline previews where possible. The same object model works across server, desktop, mobile, and kiosk — the host adapter decides how to present it.

**What needs to be built:**
- Typed object metadata in the namespace layer (localhost-provider returns type, size, preview — not just bytes)
- Viewer resolution (runtime maps object types to installed viewer capsules)
- Home as object browser (Home shows objects, not just launch buttons)
- Marketplace WebSpace (browsable catalog with install actions)

### Identity evolution

Keep `did:key` as the local foundation.
Extend it toward richer local profile coherence, persona separation, and later cross-device or chain-linked identity only when the local contract is clean.

### Protected content and stronger attestation

Encrypted capsules, remote trust, reproducible builds, TPM/TEE-backed attestation, and dDRM-like flows remain future work.
They matter, but they should not distort the core runtime contract before the local base is stable.

### AI and operator surfaces

Agent and AI provider surfaces should keep moving toward one stable runtime contract with explicit policy, identity, and budget boundaries instead of ad hoc special cases.

## How to use this file

If a statement is a current proof claim, a release note, a version-specific fact, or a machine-specific result, it does not belong here.
This file should stay useful even when the next week of implementation details changes.
