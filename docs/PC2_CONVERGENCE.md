# PC2 Convergence Notes

> Last verified from public PC2 `main`: 2026-05-29
> (`a0a910158bd67666a6d3ea2a775ce09005ba7ae7` via `git ls-remote`).
> This commit is tagged `v1.3.0` in PC2 and is the current migration baseline.
>
> This document is a Runtime translation of useful PC2 implementation patterns.
> It is not a commitment to port PC2 code or revive older PC2/Puter assumptions.

## Decision

The PC2 inventory is valuable as a convergence input, but only after it is
translated into the current ElastOS Runtime model:

`capsule -> runtime capability -> Carrier/provider plane -> provider backend`

PC2 has working product code for wallets, dDRM, IPFS availability, app
packaging, and launcher/runtime health. Those are useful references. The Runtime
must not copy PC2's older iframe, broad-session, app-visible wallet, or direct
IPFS patterns.

## Verified PC2 Code Inputs - 2026-05-29

Use PC2 `main` as the implementation reference. It is newer than the April
inventory and already includes the recent dDRM, zero-CEK, marketplace, wallet,
and Monetisation Agent S1 work.

| PC2 ref | Status | Runtime use |
|---|---|---|
| `main` / `v1.3.0` at `a0a910158bd67666a6d3ea2a775ce09005ba7ae7` | Canonical baseline | First reference for Explorer, AI Chat, wallet bridge, protected content, and Marketplace code study. |
| `release/2026-05-28-ddrm-hardening` at `0340618c` | Included in `main` | Historical checkpoint only. |
| `feat/ddrm-zero-cek-exposure` at `e80a5579` | Included in `main` | Historical checkpoint only. |
| `feature/elacity-ddrm-marketplace` | Included in `main` | Historical checkpoint only. |
| `dev/ipfs-connectivity` | Not fully merged into `main` | Reference-only for IPFS/CAR/connectivity experiments; do not use as the baseline. |
| `ai-work` | Older, not fully merged into `main` | Reference-only for AI/user-isolation lessons; `main` has the current AI service. |
| `feature/virtual-workspaces` | Older, not fully merged into `main` | Reference-only for workspace UX ideas; contains Puter-era assumptions that must not be imported directly. |

Concrete files checked on PC2 `main`:

- File/content/availability: `pc2-node/src/storage/ipfs.ts`,
  `pc2-node/src/api/storage.ts`, `pc2-node/src/api/file.ts`,
  `pc2-node/src/api/filesystem.ts`,
  `pc2-node/src/services/ContentSeedingService.ts`,
  `pc2-node/src/services/ContentIndexerService.ts`,
  `pc2-node/src/services/clusterPin.ts`, and
  `pc2-node/wasm-apps/ipfs-assemble/*`.
- AI Chat: `pc2-node/src/api/ai.ts`,
  `pc2-node/src/services/ai/AIChatService.ts`,
  `pc2-node/src/services/ai/tools/ToolExecutor.ts`, and
  `pc2-node/src/services/ai/tools/MonetisationAgentTools.ts`.
- Wallet bridge/capability vocabulary:
  `pc2-node/src/wallet-bridge/pc2-wallet-bridge.js`,
  `pc2-node/src/wallet-bridge/pc2-wallet-provider.js`, and
  `pc2-node/src/types/capabilities.ts`.
- dDRM/Marketplace: `pc2-node/src/services/wasm/WasmDdrmDecryptRuntime.ts`,
  `pc2-node/crates/ddrm-decrypt/*`, `pc2-node/src/api/storage.ts`
  Lit/Chipotle session routes, `pc2-node/src/api/chipotle-client.ts`,
  `pc2-node/data/test-apps/elacity-market/*`,
  `pc2-node/data/test-apps/elacity-creator/*`,
  `pc2-node/data/test-apps/elacity-player/*`, and
  `pc2-node/data/test-apps/ddrm-viewer/*`.

Entropy finding from the PC2 code study: the useful logic is real, but much of
it is route/app/iframe-shaped and broad-session-shaped. Runtime should lift the
protocol boundaries and acceptance fixtures, not transplant the monoliths.

## What To Reuse

| PC2 asset | Runtime translation | Why it matters |
|---|---|---|
| Wallet bridge RPC classification | `wallet-provider`, `chain-provider`, and dedicated wallet connector capsules | PC2 already separates wallet reads, signing, and network RPC. That maps to typed capability scopes and fail-closed provider operations. |
| EIP-1193 shim behavior | Connector-capsule compatibility reference only | Helpful for MetaMask/WalletConnect UX, but ordinary apps must not receive `window.ethereum` or a wallet object. |
| Runtime heartbeat protocol | Runtime/supervisor health contract reference | Schema-versioned heartbeat files are a good pattern for launchers and operators because they avoid PID-only truth. |
| IPFS cluster and supernode replication work | `content-provider` plus `availability-provider` | PC2's cluster/supernode path is a concrete backend for SmartWeb availability, not the capsule-facing contract. |
| dDRM contracts and access pipeline | `rights-provider`, `key-provider`, `decrypt-provider`, and sealed objects | The contract addresses and access questions are useful after the provider boundary is enforced. |
| WASM crates for decrypt/render/media/EVM helpers | Provider internals | These may be useful implementation modules if they remain inside provider boundaries and never grant raw authority to app capsules. |
| App manifest and signing registry ideas | Capsule publish/install registry input | Useful for future app catalog work after signed package identity, interface contracts, and install receipts are stable. |

## What Not To Port

- Do not use Puter as the Runtime product ontology. The visible front door is
  Home; Puter-era assumptions stay historical.
- Do not inject a general wallet object into app iframes. External wallets live
  in connector capsules; built-in wallets live in `wallet-provider`.
- Do not make wallet proof the Runtime principal root. Wallets are proof bindings
  on passkey-first Runtime principals.
- Do not map the wallet bridge to `did-provider`. Wallet mediation belongs in
  `wallet-provider` and connector capsules. DID providers handle device DID
  identity, typed DID verification, credentials, service endpoints, recovery
  proof checks, and future chain anchoring.
- Do not expose raw chain RPC, Kubo/IPFS APIs, Elacity SDKs, CEKs, or
  private keys to normal app, viewer, or content capsules.
- Do not treat CIDs as availability guarantees. Availability requires provider
  receipts, replication policy, repair loops, and eventually incentives.
- Do not import PC2 phase plans literally. Runtime sequencing follows the
  current roadmap: passkey authority, content availability, protected content,
  wallet/DID/node adapters, Spaces, then capsule registry.

## Current Runtime Mapping

| Runtime area | PC2 input to use | Current rule |
|---|---|---|
| Passkey/Home authority | Login UX and launcher lessons | Passkey is the Home front door. Wallets link after a Runtime principal exists. |
| Built-in wallet | Wallet bridge capability vocabulary | Built-in key material stays provider-owned and is used only after Wallet/Inbox approval. |
| MetaMask | EIP-1193 bridge behavior | MetaMask is a dedicated connector capsule. System must not contain browser-wallet authority. |
| WalletConnect | Wallet bridge and connector UX | Add only as a pinned/configured connector capsule with no unpinned CDN and no app-visible wallet object. |
| Chain provider | PC2 chain metadata and RPC classification | Expose typed proof, prepare, broadcast, lifecycle, and sync-health operations, not arbitrary RPC passthrough. |
| Content availability | PC2 Kubo/IPFS Cluster/supernode work | `elastos://content/*` is the capsule contract. IPFS/Kubo/cluster/Elacity are backends. |
| Protected content | PC2 dDRM contracts and WASM decrypt/render crates | Keep rights, key release, and decrypt/render split. Apps receive scoped output, not raw keys. |
| Runtime health | `pc2.heartbeat.v1` | Use schema-versioned, fail-closed health state for future launcher/supervisor contracts. |
| Capsule registry | PC2 app registry and signing ideas | Reuse only after Runtime package identity, interface descriptors, and install/update receipts exist. |

## Near-Term Action Items

1. Start with Explorer / Library / WebSpace browsing as the first PC2
   product migration slice. Use PC2's file-manager and content UX as reference,
   but implement it through Home/Library, principal-root storage,
   `elastos://content/*`, WebSpace mounts, and availability receipts.
2. Bring AI Chat over as a provider-backed capsule. The chat UI is a normal app
   capsule; model execution, hosted-model credentials, embeddings, and local
   context access stay in `ai-provider` / `llama-provider` / explicit hosted
   provider contracts.
3. Stage dDRM and Elacity Marketplace behind protected-content providers before
   porting Marketplace/Creator/Player/Viewer UX. The provider sequence is
   `elastos://drm/open -> content status/fetch -> rights-provider ->
   key-provider -> decrypt-provider -> scoped viewer session`.
4. Keep WalletConnect behind the wallet connector capsule contract.
5. Use PC2 wallet bridge method classification as test fixtures for
   `wallet-provider` and `chain-provider` capability mapping.
6. Treat PC2's IPFS cluster work as the first realistic `availability-provider`
   backend design, not as a reason for app capsules to call IPFS directly.
7. Use PC2 dDRM and WASM crates as protected-content provider implementation
   candidates after the fail-closed `drm/open -> rights -> key -> decrypt`
   sequence is wired.
8. Evaluate PC2 heartbeat as a runtime/operator health contract once launcher or
   supervisor integration resumes.

## Explorer UX Translation

The Explorer should preserve PC2's user experience where it is useful, but
not PC2's authority model. The Runtime target is a PC2-familiar object browser on
ElastOS rails.

The implementation gate is the current Library/Object provider contract:
preserve useful file-manager behavior where it helps users, but translate every
operation onto typed Runtime object/provider contracts instead of older
filesystem, Puter, or direct IPFS assumptions.

Keep from PC2:

- places/sidebar navigation
- breadcrumbs and current-folder state
- grid/list switching, icon tiles, details columns, and type-aware previews
- drag/drop upload, upload progress, new folder, inline rename, context menus,
  properties/details, open, download, and copy URI/CID actions
- share/publish badges and availability status such as local-only, syncing,
  network-available, and repair-needed

Do not keep from PC2:

- Puter-era wallet-address roots or username path aliases as filesystem truth
- `/null` fallback paths
- bearer-token file shortcuts as an app authority model
- direct app calls to Kubo, IPFS Cluster, Elacity APIs, raw host paths, or broad
  `localhost://Users/*`
- socket/global-state assumptions that bypass Runtime capabilities or audit

Implementation rule: add the typed object provider contract first, prove
principal isolation and protected-root writes, then build the PC2-style UI on top
of that contract.

## First Migration Slices

| Slice | User-facing target | Runtime contract | PC2 reference input | Acceptance gate |
|---|---|---|---|---|
| Explorer / Library / WebSpace | Browse, upload, download, open, rename, publish, and share files/objects from Home | Home/Library app capsule, typed object provider, principal-root storage, WebSpace mounts, `elastos://content/*`, `availability-provider` | PC2 file manager UX, IPFS/content UX, Kubo/IPFS Cluster/supernode availability work | One file can be uploaded, opened, renamed, published, shared, and proven unavailable to another principal unless shared; app cannot bypass the provider plane |
| AI Chat | Open Chat, ask a question, optionally attach a local object/document | Chat app capsule plus `ai-provider` / `llama-provider` / hosted model provider; context by object capability | PC2 AI Chat UX and provider lessons | Prompt succeeds through provider capability; missing provider fails closed; no raw model key, host HTTP credential, or filesystem path reaches app code |
| Protected content / Elacity | Browse protected content and open only when access is proven | `elastos://drm/open`, `rights-provider`, `key-provider`, `decrypt-provider`, `chain-provider`, Wallet/Inbox approvals | PC2 dDRM contracts, WASM decrypt/render/media crates, Elacity Marketplace/Creator/Player/Viewer | One pinned fixture opens for the rightful account and fails closed for another account; apps never receive raw CEKs, chain RPC, wallet RPC, Kubo/IPFS, or Elacity SDK authority |

## Verification Rule

Before importing any PC2 pattern, the implementation must name:

- the Runtime principal/session/capability it depends on
- the provider or connector capsule that owns the dangerous authority
- the app-visible API that remains protocol-agnostic
- the fail-closed test that proves apps cannot bypass the provider plane
- the source commit or pinned artifact being used as reference
