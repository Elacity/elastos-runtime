# Glossary

Lookup. Not law. If this fights `PRINCIPLES.md`, the principle wins.
Repo `docs/GLOSSARY.md` is the longer source. This file is the one agents and people should finish first: missing product words, plus the official terms in shorter form.

**Names.** ElastOS (two capitals) is this runtime. Elastos is the foundation and ecosystem. `elastos` is the binary, crate names, and URI scheme.

## People and places

**Home.** The person's front door. Passkey in, desktop out. Browser Home is `elastos gateway` at `/apps/home/`. `elastos home` is the managed user front door. Not the seed.

**Seed.** The planter. Network and signature chains. People run `install.sh` from it and are tethered at plant time. After that each Home has its own passkey, data dir, and Carrier endpoint. Not "the" Home. Shared-endpoint use is an edge, not the default.

**Install tether.** What `install.sh` binds at plant time: component and manifest trust, collaboration bootstrap, Carrier bootstrap tickets. After plant, the Home must not need the seed as identity or a chat hop.

**Principal.** Who Runtime is acting for. A human, agent, device, capsule, or provider. Sessions and tokens are issued to principals. Wallet, passkey, and DID are proofs bound to a principal, not replacements for it.

**Passkey.** How a person unlocks Home. Not a wallet. Not a DID. Does not normally expose raw key material.

**WebAuthn PRF.** Optional passkey extension that can wrap a principal data key on the client. Raw PRF output is key material. Never send it to auth routes, logs, or session storage.

**Device DID (`did:key`).** The node. Carrier signing and routing. Not the person. Does not prove a global name.

**Profile DID.** The person other Homes can name. Lives on a signed Profile. Survives device replacement. Not the passkey.

**Account DID (`did:elastos` / EID).** Future or linked global account. Portable name, credentials, publisher identity. Not required for a local passkey Home.

**Handle / name.** Display label such as `alice`. Local handles can collide. Authority binds to principal IDs, DIDs, signatures, and CIDs, never to an unverified string.

**PeerDid.** How Runtime addresses another endpoint on Carrier. Capsules never see it.

**Users / UsersAI.** Parallel lanes under one authority model. Automation is more explicit, not more ambient.

## How work moves

**Four quadrants.** Planning frame: Home, Runtime, Carrier, Blockchain. Boundaries, not four products.

**Runtime.** The trusted `elastos` binary. Isolation, signatures, capabilities, routing, audit. Everything outside it is a capsule.

**ESP.** ElastOS Shell Protocol. How Home talks to Runtime. Facts and intents. Not IPsec. Not Carrier. Not a second authority.

**Bus.** How a component capsule invokes Runtime (`elastos:bus@v1`). Not Carrier.

**Carrier.** Endpoint-authenticated off-box transport. Runtime selects it. Not the capsule API. Not authority. Proves the transport peer, not who wrote the message. Today's network plane is iroh.

**Loopback.** Same-node delivery that must use the same envelope and receipt as remote Carrier. Law. Not present as authenticated loopback on `review/collaboration-candidate` @ `1e035af`.

**Launch token.** `elastos.home.launch-token/v4`. Runtime mints it after Home is already allowed. The iframe is not the grant.

**Capability token.** One capsule, one action, one resource. Epoch, expiry, max uses. It does not prove who you are.

**Capability view.** A derived list of what a capsule can currently see. Discovery only. Every operation still needs the real token.

**HTTP.** How a browser or control plane arrives. Not the product ABI. `POST /api/provider/...` still exists and is still called. Do not add to it.

## Things that run

**Digital Capsule.** Signed portable package. App, provider, shell, agent, or sealed content. A user's document is an object until it is packaged with capsule metadata.

**Capsule.** Shorthand, usually executable. Zero ambient authority. Substrates today: WASM and microVM.

**Capsule artifact.** Immutable package: manifest, payload, signature.

**Capsule instance.** One running copy, bound to session, capabilities, and substrate.

**Capsule state.** Mutable data for that instance or user. Separate from the artifact.

**Capsule Runtime.** The per-capsule execution contract across WASM and microVM. Not the trusted node. Spans `elastos-guest`, `elastos-compute`, `elastos-crosvm`, and guest bridges.

**App (`app`).** What a person opens. Public word is App. Internal word is capsule.

**Shell.** Draws Runtime facts. Emits typed intents. Not a provider. `home-gui` and `home-cli` are shells.

**Viewer.** Opens declared content.

**Provider.** A capsule that implements a protocol other capsules consume through `elastos://` or rooted `localhost://`.

**Content (role).** Data. Optional viewer binding.

**Component.** Capsule on the Bus ABI. Shipped first-party UI Apps are still web projections, not Components.

**Web projection.** iframe UI under the same authority model. Current shipped Apps.

**Agent (capsule role).** A capsule that acts for a principal. Same capabilities as a human path. The operator CLI is not UsersAI.

**MicroVM.** crosvm/KVM on Linux, or Virtualization.framework on Mac when that path exists. Needs a hypervisor. The seed has no KVM. Default Home must stay usable without KVM.

## Names and bytes

**`localhost://`.** Local object names. The product model for "my stuff."

**`elastos://`.** Contract surface. Capability-checked. Some ops are local providers, some ride Carrier.

**CID.** Hash of the bytes. Integrity, not availability. Not a person or a name.

**IPLD.** Hash-linked object graph for manifests, signed heads, provenance, sealed descriptors, availability receipts. Not Carrier. Not storage. Not rights.

**WebSpace.** A mounted resolver surface. Not a disk folder. A mount is not a grant.

**Object.** The user's thing: document, song, photo, site. Distinct from a capsule and from a space.

## Protected content

**CEK.** Content encryption key. Encrypts the asset. May exist only inside reviewed custody and decrypt. Never in an app, Carrier, Runtime, or an ordinary receipt.

**dKMS.** Distributed key management for protected content. Threshold release of the CEK. Product rule: talks only through Runtime-selected Carrier. Written principles still allow "another substrate." Hold the product rule.

**Rights.** Signed proof a principal may open. Not the CEK.

**Availability.** Enough verified replicas or fragments exist. Open needs this and the dKMS threshold.

**Protected content provider.** Runtime-mediated `elastos://drm/*`. Apps do not get raw CEKs, wallet RPC, chain RPC, or Kubo.

**Content availability provider.** Higher-level publish/fetch/repair. Apps use `elastos://content/*`, not raw `elastos://ipfs/*`.

**FROST.** May sign classical v0 receipts. Not the long-term dKMS root.

## Network edges

**Exit.** The egress contract (`exit-provider`, `elastos://exit/*`). On the current Browser path the public DNS/TCP dialer is the helper `browser-local-exit`, not the capsule. Not Carrier. Not Home. Not authority.

**Net.** Lower guest-network policy and plumbing. Ordinary Apps should not need TAP or `sudo`.

**Browser engine.** Hosted engine that actually paints web content. May run in a microVM. If this node has no KVM, Runtime selects an engine hosted elsewhere over Carrier. The Browser capsule must not name that host.

**Browser (capsule).** The App people open. Routes through Browser / Net / Exit / Engine. Works. Unstable and slow on Mac. Other hosts untested. Still sets `webrtc_remote_display` in product code. Repo says the product browser is not complete; hosted Selkies is a proof path.

**Guest network.** Explicit compatibility mode with conventional guest networking. Not the default for ordinary Apps.

**TAP.** Host NIC used only in that compatibility mode. Runtime-owned. Not an App ABI.

**iroh.** Current Carrier network-plane implementation.

**Boson.** Future Elastos-native Carrier transport under the same abstraction.

**WireGuard.** Not a Runtime or capsule contract. Absent from Principles, glossary, Carrier, and Architecture on this SHA. Old dKMS meshes are evidence only.

## Product surfaces

**System.** The operating surface a person can see. Real controls, not placeholder categories.

**Library.** Provider-backed objects. Mutation stays in object-provider, not in Home chrome.

**Inbox.** Approvals and incoming work under the same capability model.

**Chat Room.** Product Chat. The capsule is `chat-room`. Not the retired Carrier gossip chat.

**Wallet.** Proof, account-link, and approval. Apps do not get raw wallet RPC.

## Stale words

**`POST /api/provider`.** Still live (Library, wallet, System). Not the capsule contract. Do not teach it as the ABI.

**Carrier chat / agent gossip.** Drawn in `docs/CARRIER.md`. Retired. `components.json` has no `chat` or `agent`.
