# PC2 Convergence Notes

> Last verified from public PC2 `main`: 2026-05-06.
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

1. Keep WalletConnect behind the wallet connector capsule contract.
2. Use PC2 wallet bridge method classification as test fixtures for
   `wallet-provider` and `chain-provider` capability mapping.
3. Treat PC2's IPFS cluster work as the first realistic `availability-provider`
   backend design, not as a reason for app capsules to call IPFS directly.
4. Use PC2 dDRM and WASM crates as protected-content provider implementation
   candidates after the fail-closed `drm/open -> rights -> key -> decrypt`
   sequence is wired.
5. Evaluate PC2 heartbeat as a runtime/operator health contract once launcher or
   supervisor integration resumes.

## Verification Rule

Before importing any PC2 pattern, the implementation must name:

- the Runtime principal/session/capability it depends on
- the provider or connector capsule that owns the dangerous authority
- the app-visible API that remains protocol-agnostic
- the fail-closed test that proves apps cannot bypass the provider plane
- the source commit or pinned artifact being used as reference
