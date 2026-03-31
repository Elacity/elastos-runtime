# ElastOS Runtime Roadmap

This file is for direction only.

For active work, see [TASKS.md](TASKS.md).
For current state, see [state.md](state.md).

---

## Mission

Make this repository the trusted local runtime layer of ElastOS:
- execute capsules predictably
- expose one coherent local object model
- use Carrier-first off-box transport
- keep release, install, update, share, and site flows boring
- give PC2 a stable front door instead of a demo-only shell

## Non-Goals

This repo is not the whole SmartWeb stack.
It is not the blockchain/payment layer, the whole PC2 product, or the full Carrier/Boson program.
It should integrate with those surfaces without pretending to own them.

## Near-Term Direction

### 1. One runtime contract for executable capsules

Converge native, WASM, and microVM capsules on one explicit contract for:
- identity bootstrap
- capability acquisition
- Carrier access
- localhost storage access
- interactive TTY ownership
- home/exit signaling

Do not keep multiple half-compatible runtime stories alive.

### 2. Make PC2 a boring front door

PC2 should stay inside one owned interactive session and make the main user path obvious:
- launch
- navigate
- open a surface
- return home cleanly

The runtime should support that without CLI detours, TTY confusion, or host-specific guesswork.

### 3. Keep release, install, and update on one truthful path

The product path should remain:
- signed install
- trusted source configuration
- plain `elastos update`
- fail-closed behavior when trust is missing

Operator/debug paths can exist, but they must stay explicit and secondary.

### 4. Keep the rooted object model coherent

The runtime should keep strengthening the relationship between:
- `localhost://...`
- `elastos://...`
- WebSpace-style mounted views

The goal is one stable object model, not a pile of one-off path conventions.

The object model should lead with human concepts, not implementation seams.
Users should primarily think in terms like people, spaces, sites, shares, apps, and agents rather than providers, runtimes, gateways, or transport details.

One concept should survive across multiple realizations.
If something appears in PC2, under `localhost://...`, as an `elastos://...` object, or through a public URL, it should still read as the same underlying thing rather than four different products glued together.

Keep the ontology small and flexible.
Prefer a minimal set of durable concepts and role-based views over a deep rigid hierarchy that will be wrong once the system grows.

### 5. Build native collaboration around Carrier and runtime objects

Native `Chat` is the proving surface.
IRC and other compatibility surfaces may help earn the runtime contract, but they should not replace the target architecture.
Long term, collaboration should be Carrier-first, capability-gated, and built on runtime/provider boundaries rather than classic centralized web-server assumptions.

### 6. Keep site and publication flows local-first

`MyWebSite`, publication, release channels, and public serving should keep moving toward one coherent local-first story with explicit promotion and rollback.
The runtime should own the object/state model cleanly, even when gateways or public edges sit in front of it.

## Later Direction

### Cross-platform runtime and host adapters

The long-term shape is one ElastOS contract above multiple host adapters. The runtime, the capability model, the namespace, and the capsule contract are the same everywhere. What changes is how the host presents capsules to the user.

**Host adapter modes:**
- **Server / headless:** Runtime serves capsule UIs over HTTP. PC2 is a web dashboard accessed from any browser. No local GPU or window manager required. This is the home server, NAS, or cloud deployment model.
- **Desktop (Linux, Windows, macOS):** Runtime opens capsule UIs in browser tabs or native windows. PC2 is the local launcher. GPU is available for rendering. Capsules that produce web UI open in the browser; terminal capsules open in terminal windows.
- **Mobile (Android, future iOS):** Runtime is a background service. The launcher is a native app. Capsules render in embedded webviews. The capability model gates sensor, storage, and network access the same way it does on desktop.
- **Kiosk / dedicated device:** Runtime owns the full display. PC2 IS the desktop environment. Capsules launch fullscreen or in managed windows. This is the Jetson, set-top box, or dedicated appliance model.

**Capsules don't know which host adapter they're on.** A capsule that serves HTML on its HTTP port works identically on a headless server (proxied through the runtime), a desktop (opened in the browser), or a mobile device (rendered in a webview). The Carrier bridge, provider access, and capability model are identical regardless of host.

Linux remains the truthful full-runtime baseline. Other platforms should be earned without pretending to offer Linux/KVM parity everywhere. The server/headless mode is the simplest first step toward cross-platform — it requires only the runtime binary and a browser, no platform-specific UI integration.

### Native object model and content-first design

The compatibility path (packaging existing web apps as capsules) gets existing software into ElastOS. But the native app model should be designed from first principles around **objects, not applications**.

**Core idea: everything is a typed object in the namespace.**
A photo is not `~/Photos/IMG_001.jpg`. It is `localhost://Users/self/Photos/IMG_001` — a typed object with metadata, preview capability, provenance, and access control. The runtime knows it is an image. PC2 can render a preview without launching a capsule. A capsule requests access to `localhost://Users/self/Photos/*` and gets typed objects back, not raw bytes.

**Apps don't own content, they view it.**
The `viewer` field in capsule.json already points this direction — gba-ucity is a data capsule, gba-emulator is its viewer. Scale that up: a PDF is a data object, a PDF viewer capsule renders it. An image is a data object, a gallery capsule renders it. The runtime resolves which viewer handles which type. Users open objects, not apps. The runtime picks the viewer.

**The homescreen is the object browser.**
PC2 evolves from "launch apps" to "navigate your objects." The natural tabs become:
- **Home** — recent objects, pinned spaces, activity stream
- **People** — identity objects (DIDs), conversations, shared spaces
- **Spaces** — rooted namespaces (Users, Public, MyWebSite, WebSpaces)
- **Apps** — installed capsule viewers and tools
- **System** — services, updates, trust configuration

Users navigate objects. Capsules appear when an object needs one.

**The browser is a capsule, not the platform.**
A web browser capsule gets `localhost://Users/self/Bookmarks/*` and explicit outbound network capability. It is one viewer among many, not the runtime itself. This is the inversion from ChromeOS: instead of everything running in the browser, everything runs in the runtime and the browser is one sandboxed capsule.

**The marketplace is a WebSpace.**
`localhost://WebSpaces/Marketplace` resolves to a typed catalog of published capsules with signatures, descriptions, versions, and install actions. Installing from the marketplace is `elastos capsule install <name>`. The marketplace capsule provides the UI; the runtime provides the trust verification and signature checking.

**Digital assets are typed by the namespace.**
The resolver layer knows:
- `localhost://Users/self/Photos/*` → image objects
- `localhost://Users/self/Music/*` → audio objects
- `localhost://Users/self/Documents/*` → document objects
- `localhost://Users/self/Models/*` → 3D model objects
- `localhost://Users/self/Videos/*` → video objects

Each type has a default viewer capsule. The runtime dispatches. PC2 renders inline previews where possible. The same object model works across server, desktop, mobile, and kiosk — the host adapter decides how to present it.

**What needs to be built:**
- Typed object metadata in the namespace layer (localhost-provider returns type, size, preview — not just bytes)
- Viewer resolution (runtime maps object types to installed viewer capsules)
- PC2 as object browser (Home tab shows objects, not just launch buttons)
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
