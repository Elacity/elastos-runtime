# Chain Provider

`chain-provider` is the first blockchain-quadrant provider slice. It gives
capsules typed access to Elastos and node-backed chain networks through the
runtime/provider plane:

`capsule -> runtime capability -> elastos://chain/* -> chain-provider -> backend`

Capsules do not receive raw RPC URLs, node ports, wallet SDK objects, or private
keys. The provider owns protocol details; the runtime owns capabilities,
sessions, registration, and audit.

## Current Networks

The default network list is production-only. Non-production and identity-chain
networks stay out of the user-facing System surface until those flows have a
concrete wallet/DID journey.

| Network ID | Chain | Kind | Chain ID | Current backend |
|------------|-------|------|----------|-----------------|
| `ela-mainnet` | Elastos Mainchain | REST | n/a | official Elastos mainchain explorer API |
| `esc-mainnet` | Elastos Smart Chain | EVM JSON-RPC | `20` | official Elastos RPC endpoint |
| `base-mainnet` | Base | EVM JSON-RPC | `8453` | public Base RPC endpoint used by PC2 (`https://mainnet.base.org`) |
| `btc-mainnet` | Bitcoin | REST | n/a | typed mempool.space backend |

All default networks support `status`, `block_number`, and `sync_health` reads.
Operators can override the default Base backend with `BASE_RPC_URL`, and can
replace `btc-mainnet` with a `bitcoin_core_rpc` network in provider config when
a local Bitcoin Core node is available.

## Provider Operations

Supported operations are intentionally narrow:

- `networks`: list supported networks without exposing backend RPC URLs.
- `status`: verify upstream EVM chain ID or read typed mainchain/Bitcoin status.
- `block_number`: read latest EVM block number or mainchain/Bitcoin height.
- `sync_health`: read typed sync state for an EVM node, Bitcoin Core node, or configured REST backend.
- `balance`: read an EVM account balance or a Bitcoin REST address balance.
- `contract_call`: perform a validated EVM `eth_call` read for Browser dapp compatibility; requires network, `to`, hex data, and block tag validation and never exposes the backend RPC URL.
- `estimate_gas`: perform a validated EVM gas preflight for Browser dapp compatibility; requires network, selected `from`, `to`, value, and hex data validation and does not sign or broadcast.
- `transaction`: read an EVM transaction by hash.
- `receipt`: read an EVM transaction receipt by hash.
- `has_access_by_content_id`: typed rights-read for protected content; validates network, configured contract, content ID, subject, and right, then calls an approved ABI selector. Capsules cannot mint this as a capability. The v1 Runtime evaluator uses `protected_content_rights_evidence`.
- `protected_content_rights_evidence`: Runtime-internal typed evidence for the protected-content rights evaluator. Request is the signed Runtime release operation only. Not capsule-mintable.
- `proof`: create typed evidence over status or sync-health without exposing the backend.
- `erc1271_is_valid_signature`: verify a smart-account signature with the typed ERC-1271 `isValidSignature(bytes32,bytes)` ABI without exposing arbitrary `eth_call`.
- `prepare_transaction`: build a typed unsigned EIP-155 legacy EVM transaction intent with nonce, gas price, and gas limit resolved by the provider; the intent requires wallet approval before signing.
- `broadcast_transaction`: broadcast an already signed EVM transaction through the provider boundary.
- `node_lifecycle`: report and persist typed node lifecycle status, and run start/stop/restart only for explicit operator-approved loopback supervisor config.

There is no arbitrary JSON-RPC passthrough. Browser compatibility operations
such as `contract_call` and `estimate_gas` are still typed provider operations:
they validate the request, run under scoped capability resources, emit Browser
audit records at the gateway, and never expose raw RPC URLs or node ports.

## Capability Schema

The runtime maps provider requests to scoped resources before they reach the
provider. This keeps discovery, reads, proofs, transaction preparation,
broadcast, and revocation separable.

| Scope | Resource | Current status |
|-------|----------|----------------|
| Discovery | `elastos://chain/meta/networks` | Implemented |
| Read | `elastos://chain/<network>/status` | Implemented |
| Read | `elastos://chain/<network>/block_number` | Implemented |
| Sync health | `elastos://chain/<network>/sync_health` | Implemented |
| Read | `elastos://chain/<network>/balance` | Provider implemented and exposed to System/Wallet through the browser gateway |
| Browser read | `elastos://chain/<network>/contract_call` | Provider implemented; Browser gateway maps EIP-1193 `eth_call` here for dapp token reads |
| Browser preflight | `elastos://chain/<network>/estimate_gas` | Provider implemented; Browser gateway maps EIP-1193 `eth_estimateGas` here before transaction approval |
| Read | `elastos://chain/<network>/transaction` | Provider implemented and exposed through the Browser gateway |
| Read | `elastos://chain/<network>/receipt` | Provider implemented and exposed through the Browser gateway |
| Rights read | `elastos://chain/<network>/rights/has_access_by_content_id` | Provider calls configured typed ABI and fails closed when the contract/selector is not configured |
| Proof | `elastos://chain/<network>/proof` | Provider implemented for status/sync-health evidence |
| ERC-1271 proof | `elastos://chain/<network>/proof/erc1271` | Provider implemented for typed smart-account signature verification |
| Transaction prepare | `elastos://chain/<network>/prepare_transaction` | Provider implemented for signable typed EIP-155 legacy EVM transaction intents |
| Transaction broadcast | `elastos://chain/<network>/broadcast_transaction` | Provider implemented for signed EVM transaction submission |
| Node lifecycle | `elastos://chain/<network>/node_lifecycle` | Provider status persists typed state and is System-gateway covered; control actions require explicit loopback supervisor config |
| Revoke | `elastos://chain/<network>/revoke` | Planned |

Write or broadcast operations must remain behind capability scope, wallet
approval, audit, and fail-closed tests. Apps still never receive raw RPC URLs or
node ports.

`prepare_transaction` is not a raw RPC proxy. For EVM networks it validates the
from/to/value/data request, reads `eth_getTransactionCount`, `eth_gasPrice`, and
`eth_estimateGas` inside the provider boundary, then returns a signable
`elastos.chain.unsigned_transaction_intent/v1` payload. The wallet-provider can
sign that payload only after Runtime approval; ordinary apps do not receive the
node RPC URL or wallet key material.

## Rights Method Configuration

Protected-content rights reads are opt-in per network. A caller may request a
contract, but `chain-provider` only calls it when that exact contract is already
configured for the network.

Current supported ABI:

```json
{
  "id": "has_access_by_content_id",
  "contract": "0x0000000000000000000000000000000000000001",
  "abi": "has_access_by_content_id_address_bytes16",
  "selector": "0x12345678"
}
```

The provider encodes:

```text
hasAccessByContentId(address subject, bytes16 contentId) -> bool
```

The selector is configured explicitly so the provider does not need arbitrary ABI
loading or contract SDKs. Missing config, mismatched contracts, malformed
selectors, invalid subjects, invalid `bytes16` content-access IDs, and malformed
return values all fail closed.

## Backend Policy

The default Elastos backends use official public Elastos endpoints. The default
Base backend uses the same public Base RPC endpoint currently configured in the
PC2 stack (`https://mainnet.base.org`). The default Bitcoin backend uses typed
mempool.space reads for status and height. This keeps the hosted development
gateway functional without running heavy blockchain daemons on rented
infrastructure.

For operator-owned Bitcoin Core, configure a `bitcoin_core_rpc` network with a
loopback endpoint such as `http://127.0.0.1:8332`. If the node requires
credentials, set `BITCOIN_RPC_USER` and `BITCOIN_RPC_PASSWORD` in the
chain-provider runtime environment. Capsules still never receive that URL or
those credentials.

Additional local nodes should be added behind the same provider contract:

- ELA mainchain node
- ESC node
- Base node

When local nodes are added, capsules still call `elastos://chain/*`. Only the
provider backend changes.

Do not run heavy blockchain node daemons on rented infrastructure by default.
Operators must confirm host policy, disk, traffic, and abuse constraints before
enabling local node backends. The default provider proxy path avoids that risk.

## Node Lifecycle State

`node_lifecycle` status records a small provider-owned state file under the
ElastOS data directory. The state is intentionally typed and minimal: network
ID, lifecycle state, managed flag, first-seen time, and updated time. It does
not persist or return backend RPC URLs, loopback ports, credentials, or node
process handles.

The status response separates persisted lifecycle facts from runtime control
capability:

- `state`: typed lifecycle classification (`not_configured`,
  `external_loopback`, `managed_local`, or `remote_backend`).
- `managed`: persisted provider classification for the backend, retained with
  the lifecycle timestamps.
- `first_seen_at` / `updated_at`: persisted status timestamps.
- `control_available`: `true` only when the backend is loopback and an
  operator-approved supervisor command set exists for that network.
- `control_reason`: human-readable reason control is unavailable or configured.

Start, stop, and restart use explicit argv command config. The provider never
accepts shell strings, never returns the command path, and never persists or
returns raw node RPC URLs, credentials, ports, or process handles. Remote
backends still fail closed for lifecycle control even if supervisor config is
present.

System may render Start, Stop, and Restart controls only from a
`node_lifecycle` response where `control_available=true`. Public or remote RPC
networks remain status-only in the UI.

Runtime gateway requests for Start, Stop, and Restart are treated as external
effects. They must be initiated by System authority, executed through the chain
provider, and recorded as `chain.node_lifecycle.*` audit events for the
requested and completed/failed phases. Status reads remain typed provider reads
and do not expose raw node RPC URLs or supervisor command details.

Example operator config shape:

```json
{
  "node_supervisor": {
    "networks": {
      "btc-mainnet": {
        "start": { "program": "/usr/bin/systemctl", "args": ["--user", "start", "bitcoin"] },
        "stop": { "program": "/usr/bin/systemctl", "args": ["--user", "stop", "bitcoin"] },
        "restart": { "program": "/usr/bin/systemctl", "args": ["--user", "restart", "bitcoin"] },
        "timeout_ms": 15000
      }
    }
  }
}
```

## Verification

Use these checks for the current slice:

```bash
cargo test --manifest-path capsules/chain-provider/Cargo.toml
cargo clippy --manifest-path capsules/chain-provider/Cargo.toml -- -D warnings
cargo test -p elastos-server --lib --manifest-path elastos/Cargo.toml
bash scripts/check-wci-alignment.sh
```
