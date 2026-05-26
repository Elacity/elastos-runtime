# Wallet Provider

`wallet-provider` is the blockchain-quadrant authority boundary for wallet
proofs, account links, approvals, typed signatures, and transaction requests.
It is not the Runtime root of identity and it is not an app SDK.

The contract is:

`capsule -> runtime capability -> elastos://wallet/* -> wallet-provider -> wallet backend`

Capsules never receive private keys, browser wallet objects, raw wallet RPC,
node RPC, seed phrases, or provider SDK handles. A wallet address is a proof
binding on a Runtime principal, not the principal itself.

## Authority Model

Runtime authority comes first:

1. Runtime principal
2. Verified proof binding
3. Short-lived session grant
4. Scoped capability
5. Provider-mediated effect
6. Signed audit event

Wallets, Essentials, EVM accounts, BTC accounts, EID, and UniversalX are proof
or signing adapters behind that model. They must not mint Runtime principals,
sessions, or privileged capabilities directly.

PC2's wallet bridge is useful reference material for method classification:
wallet reads, wallet signing, and network RPC are separate authority classes.
Runtime should reuse that classification as provider tests and connector
behavior, not by injecting a broad EIP-1193 wallet object into ordinary app
capsules. Browser wallets belong in dedicated connector capsules such as
`wallet-metamask` and `wallet-unisat`; built-in wallet authority belongs in
`wallet-provider`. External wallet links carry an explicit `connector_id`, and
external approval completion fails closed if a different connector tries to
finish the request. Home and System do not initiate wallet-link ceremonies
directly.

## User-Facing Wallet Ontology

The product surface should present one Wallet, not separate wallet products
named after connector tools such as MetaMask, WalletConnect, Bitcoin signing,
or the built-in key store. The user model is:

- **Account**: a balance/address surface for one network or account group.
- **Approval method**: the way an action is authorized, such as passkey,
  MetaMask, WalletConnect, Essentials, Ledger, or a Bitcoin wallet signature.
- **Connected wallet/device**: a specific external tool that can serve as an
  approval method.

Implementation nouns remain stricter: provider code may use `signer`,
`connector_id`, proof binding, account ID, and chain namespace. UI copy should
prefer Wallet, Account, Approval, and Connected wallet unless the user is in a
specialized connector flow where the exact external tool matters. Connector
capsules are on-demand approval surfaces; they should not compete with the
single Wallet mental model.

The visible Home launcher surface is `Wallet`. It summarizes accounts, native
balances, pending requests, defaults, and approval methods. Connector capsules
such as MetaMask, UniSat, and WalletConnect remain launchable only as approval methods
from Wallet flows, not as separate top-level wallet products. Built-in
Bitcoin accounts live beside EVM accounts in Wallet; external Bitcoin approval
tools remain connector methods. Browser extensions that do not inject into a
sandboxed iframe, such as UniSat in some profiles, may be opened as a top-level
connector window; that window still uses Runtime wallet-link and approval routes
and does not expose extension authority to ordinary capsules.

## Storage

Wallet state belongs in runtime-managed provider storage, not browser local
storage and not app capsule storage:

`localhost://ElastOS/SystemServices/Wallet/...`

The current implementation stores linked-account metadata, default-account
preferences, encrypted managed EVM wallet envelopes, pending SIWE proof
challenges, and typed wallet approval requests in `wallet-state.json`. Account
records and defaults are keyed by `principal_id`; proof challenges are
short-lived, single-use, and bind the exact challenge resource.
`request_signature` records a pending approval for an allowed intent and returns
no signature. Every signature request must name `chain_namespace + intent`. If
the caller omits `account_id`, the provider resolves the principal's explicit
default for that chain and intent or fails closed.

Built-in wallet keys are provider-owned and encrypted at rest under
`localhost://ElastOS/SystemServices/Wallet/wallet-key.hex`. They are
passkey-controlled, not passkey-derived: a long-lived Wallet/System/Inbox launch
token may list and review requests, but every built-in signature, transaction
send, account deletion, and Wallet recovery-key export/import must include a
fresh passkey-bound Home token before the provider uses or changes key material.
App capsules never receive the private key, wallet object, or raw wallet RPC.
External wallets still use the same approval request and completion receipt
shape, but the final signature is completed by the connector capsule that owns
the linked account.

Full Recovery Kits and individual Wallet recovery keys are intentionally
different. `elastos.recovery-kit/v1` restores the encrypted user root and every
recoverable built-in Wallet key included in the exported bundle. External
wallets such as MetaMask, WalletConnect, Ledger, Essentials, and UniSat restore
only their links/metadata because their private keys stay in those wallets.
`elastos.wallet.recovery-key/v1` remains an advanced per-account escape hatch
for one built-in wallet key. An old managed account whose key cannot decrypt on
the current Runtime must remain fail-closed until a full bundle or that account's
Wallet recovery key is imported or a replacement account is created.

Future DID anchoring may publish recovery or credential metadata, but local
Runtime state remains the operational source for current sessions. DID-backed
recovery proofs belong behind `did-provider` and Recovery Kit import authority,
not in approval-method connector capsules. Current import consumes a DID proof
only as a provider-verified protector check for an existing recovered root;
DID-only recovery still needs a DID-envelope unwrap/rewrap path.

## Operations

The provider surface is intentionally narrow:

| Operation | Capability resource | Purpose |
|-----------|---------------------|---------|
| `status` | `elastos://wallet/meta/status` | Report provider version and configured wallet adapters |
| `challenge` | `elastos://wallet/proof/challenge` | Create a Runtime-bound wallet proof challenge |
| `bitcoin_challenge` | `elastos://wallet/proof/bip322/challenge` | Create a Runtime-bound Bitcoin ownership challenge |
| `verify_proof` | `elastos://wallet/proof/verify` | Verify the currently supported EVM SIWE proof against the issued challenge |
| `verify_bip322_proof` | `elastos://wallet/proof/bip322/verify` | Verify the currently supported Bitcoin BIP-322 proof against the issued challenge |
| `verify_contract_proof` | `elastos://wallet/proof/verify_contract` | Consume an issued SIWE challenge only after chain-provider verifies ERC-1271 smart-account signature validity |
| `create_managed_account` | `elastos://wallet/account/create_managed` | Create or return a passkey-controlled built-in EVM account |
| `link_account` | `elastos://wallet/account/link` | Attach a verified wallet proof binding to a principal |
| `accounts` | `elastos://wallet/account/list` | List linked accounts visible to the current principal |
| `revoke_account` | `elastos://wallet/account/revoke` | Revoke a linked proof binding and related grants |
| `rename_account` | `elastos://wallet/account/rename` | Rename a principal-scoped account label |
| `export_managed_secret` | `elastos://wallet/account/export_secret` | Export a built-in account recovery key after fresh passkey verification |
| `import_managed_secret` | `elastos://wallet/account/import_secret` | Import a built-in account recovery key after fresh passkey verification |
| `set_default_account` | `elastos://wallet/account/set_default` | Select a principal-scoped default linked account for one chain and signing intent |
| `default_account` | `elastos://wallet/account/default` | Resolve the selected account without exposing wallet authority to the caller |
| `request_signature` | `elastos://wallet/<chain_namespace>/sign/<intent>` | Request explicit approval for a typed signing intent on a named chain |
| `approval_requests` | `elastos://wallet/approval/list` | List wallet approval requests for the current principal |
| `reject_approval` | `elastos://wallet/approval/reject` | Reject a pending wallet approval request |
| `approve_approval` | `elastos://wallet/approval/approve` | Approve a pending request and create a wallet handoff |
| `complete_approval` | `elastos://wallet/approval/complete` | Complete an approved request with a provider-owned signature receipt |
| `sign_approved` | `elastos://wallet/approval/sign_approved` | Execute an approved request with a provider-owned managed wallet key |

The runtime resource mapper must reject unknown wallet operations and
`request_signature` calls without a validated `chain_namespace + intent`.
Typed transaction prepare/broadcast belongs to `chain-provider`; wallet-provider
only owns proof bindings, approval state, managed signing, and connector
completion receipts.

Wallet owns accounts, approval methods, built-in send, and recovery-key
actions. `POST /api/apps/wallet/wallet/managed` creates or returns the
principal's built-in managed wallet accounts for the supported default
networks: ESC, Base, and Bitcoin mainnet. The same provider-owned key is reused
across EVM namespaces so ESC and Base show one coherent built-in wallet address;
Bitcoin uses a separate provider-owned P2WPKH key scope instead of reusing the
EVM key.
`POST /api/apps/wallet/wallet/default` selects the principal's default linked
wallet for a chain and intent. The setting lives in `wallet-provider`; Wallet
only renders the choice and cannot take over MetaMask, WalletConnect, or the
built-in wallet authority.
Inbox is a review surface: it can show and reject pending wallet requests, but
built-in signing routes back to Wallet so the user performs a fresh passkey
ceremony at the moment of use. `POST
/api/apps/wallet/wallet/approvals/:request_id/approve` approves managed wallet
requests only when a fresh passkey-bound Home token is supplied; the provider
then signs inside the wallet boundary and stores a receipt. External injected
wallets are handled by dedicated connector capsules. The MetaMask connector uses
`/api/apps/<wallet-connector>/wallet/approvals/*` to review, receive the typed
handoff message, ask its wallet backend to sign, and complete the provider
receipt. The route is generic, but only explicitly allowlisted connector
capsules can use it. Wallet and Inbox can reject pending requests.
App capsules still cannot call wallet RPC or receive signatures without the
provider approval path.

For built-in EVM transaction requests, wallet-provider only signs
`elastos.chain.unsigned_transaction_intent/v1` payloads produced by
chain-provider with `transaction_type=eip155_legacy`, nonce, gas price, gas
limit, chain ID, from, to, value, and data already bound. Incomplete or
cross-chain transaction intents fail closed before an approval request is
created. External wallet transaction signing remains connector-owned; a normal
MetaMask SIWE link is not treated as transaction-signing support until the
connector has a real transaction handoff.

For built-in Bitcoin proof requests, wallet-provider only signs
`elastos.wallet.bitcoin_bip322_request/v1` payloads that bind the managed
P2WPKH address, the Bitcoin mainnet BIP-122 namespace, and an exact Runtime
Bitcoin challenge resource. Arbitrary messages and non-Runtime challenges fail
before approval.

External Bitcoin proof requests use the same approval contract, but the Wallet
surface owns the final handoff. It shows the exact Runtime BIP-322 message,
lets the user copy it into a compatible Bitcoin wallet, accepts the returned
signature, and completes the approval only after wallet-provider verifies that
signature against the linked P2WPKH address and the stored Runtime challenge.

Wallet price data is also treated as an external effect. The temporary
CoinGecko HTTP source is disabled until Wallet raises an Inbox request and an
admin approves the local price-source policy. Actual HTTP fetch attempts then
emit `wallet.price_source.fetch.*` audit events for requested, completed, or
failed/blocked use. The target remains a typed oracle/price provider with
signed receipts instead of a raw HTTP dependency.

There is no `sign(data)` operation. Signing intents must bind principal,
proof binding, capsule, provider, chain/network, resource, nonce, expiry, and
reason. The account is either the principal's explicit default for the named
chain and intent or an explicit account that belongs to that same chain. Valid
intents include auth challenge, capability grant, credential, publish envelope,
transaction intent, Bitcoin BIP-322 proof, and revocation.

## First Shippable Slice

The default wallet path is now:

`passkey unlock -> open Wallet -> create built-in managed accounts -> view native balances -> select default -> app requests typed signature for chain + intent -> Wallet/Inbox review -> fresh passkey approval in Wallet -> wallet-provider signs inside its boundary -> signed audit/receipt`

The optional injected-wallet path is:

`passkey unlock -> open approval-method connector capsule -> connect EVM wallet -> verify Runtime SIWE proof -> link account + connector_id to the existing principal -> issue scoped connector capability -> audit -> revoke -> prove replay/expiry/wrong-chain/wrong-origin/wrong-connector fail closed`

The built-in Bitcoin proof path is now:

`passkey unlock -> create built-in managed Bitcoin wallet -> Runtime issues BIP-322 challenge -> app requests typed bitcoin_bip322_proof -> Wallet/Inbox review -> fresh passkey approval in Wallet -> wallet-provider signs inside its boundary -> signed audit/receipt`

The optional external Bitcoin proof path uses the same connector boundary:

`passkey unlock -> open UniSat -> request BIP-322 challenge -> sign the exact Runtime challenge -> verify through wallet-provider -> link BTC proof binding to the existing principal -> audit -> revoke`

For approval-gated Bitcoin proof signing after a BTC address is linked:

`app requests typed bitcoin_bip322_proof -> Wallet/Inbox review -> UniSat shows the exact Runtime BIP-322 message for external BTC accounts -> wallet-provider verifies and records a signed receipt`

The provider issues and verifies the SIWE proof challenge, and the browser EVM
wallet-link route uses that provider only after an active passkey-backed Runtime
session exists. Runtime links the wallet proof binding to that existing principal
and audits/revokes the link; the wallet proof must not mint a Home session by
itself. SIWE messages preserve the wallet-reported address display form for
wallet UX compatibility while Runtime verification normalizes addresses for
cryptographic comparison and proof IDs. Wallet approvals now have review,
approve/reject, managed signing,
dedicated MetaMask and UniSat connector handoff completion, connector-bound receipt
storage, and signed runtime audit scaffolding. An approval that expires before
managed signing or external handoff completion fails closed instead of becoming
a stale signing authority.

WalletConnect is a dedicated connector capsule (`wallet-walletconnect`), not a
wallet-provider SDK backend and not app-owned session state. `wallet-provider`
owns proof verification, linked accounts, approval state, receipts, and audit;
the connector owns only Reown/AppKit browser UX plus the operator-pinned local
adapter. Mode A is the first target: ElastOS acts as the dApp and external
wallets sign Runtime challenges or requests. Mode B, where external dApps treat
ElastOS as a WalletConnect wallet, is a later security milestone. Internal
capsules do not use WalletConnect; they call `elastos://wallet/*`.

`wallet-walletconnect` remains invisible unless the runtime finds
`elastos.walletconnect.connector/v1` config plus a local hashed Reown/AppKit SDK
asset. A configured launch can read only the narrow
`/api/apps/wallet-walletconnect/wallet/config` contract for its project ID and
local SDK asset path. The dormant connector UI imports only that pinned local
asset and expects a stable adapter export named `connectWalletConnectEvm`, which
returns an EIP-1193 provider to the connector capsule.
`scripts/vendor-walletconnect-adapter.sh` builds that adapter from exact package
versions, and `scripts/configure-walletconnect-connector.mjs` pins a reviewed
local adapter by sha256. Until a real Reown project ID and reviewed adapter are
installed into the runtime data dir, the connector remains hidden and
unroutable. Do not commit a bundled default Reown Project ID; official
deployments and independent operators pin their own runtime config. Essentials
and ELA signing should start from that connector contract because the Essentials
DID toolkit is WalletConnect-backed;
do not add a visible Essentials surface until the pinned connector exists.
Essentials, ELA, EID, BTC BIP-322, and UniversalX should reuse the same
proof-binding shape.
UniSat is the first dedicated browser Bitcoin connector because its injected API
documents `requestAccounts` and BIP-322 simple `signMessage` support. The first
Runtime proof class remains Bitcoin mainnet native P2WPKH only; UniSat users
must select a native SegWit `bc1q...` account until Taproot and other script
types have pinned test vectors and provider verification.

Do not add a BTC signing button to the MetaMask connector unless MetaMask exposes
and documents a BIP-322-capable dapp API for Bitcoin accounts. MetaMask Bitcoin
account support by itself is not enough; Runtime needs a signed proof that
`wallet-provider` can verify against the exact Runtime challenge. Until that
exists, MetaMask remains the EVM connector, Wallet owns built-in Bitcoin
accounts, and UniSat owns external BIP-322 proof-link handoff.

ERC-1271 smart-account SIWE proofs are now supported through a two-provider
sequence: `chain-provider` verifies `isValidSignature(bytes32,bytes)`, then
`wallet-provider` consumes the matching Runtime challenge only if the typed
chain proof binds the same contract address, chain ID, message hash, and
signature hash.

Bitcoin ownership proofs use the same Runtime-first shape. Browser-facing
`/api/auth/btc/challenge` and `/api/auth/btc/verify` routes are connector-token
scoped; System/Home tokens cannot create or verify BTC wallet links. Internally,
`bitcoin_challenge` issues a short-lived, single-use challenge for a Bitcoin
address, and `verify_bip322_proof` consumes it only when the signed message
matches the stored challenge exactly. The first supported proof class is
BIP-322 simple for Bitcoin mainnet native P2WPKH addresses, exposed as
`bip122:000000000019d6689c085ae165831e93` with proof type
`bip322_simple`; the built-in managed Bitcoin account uses
`managed_btc_p2wpkh` when it signs the same Runtime-bound proof after approval.
Unsupported networks, unsupported address scripts, malformed witnesses,
replayed challenges, expired challenges, wrong-message signatures, and
non-Runtime managed proof messages fail closed. Legacy Bitcoin message signing
is not accepted for privileged capabilities until it has an explicit weaker
proof class.

Broader Bitcoin script support is intentionally deferred. Adding P2SH, P2WSH,
Taproot, multisig, or hardware-wallet-specific BIP-322 behavior needs pinned
wallet compatibility, script-specific vectors, and connector UX before it can
grant Runtime capabilities. Until then, native P2WPKH is the only accepted
Bitcoin proof class.

## Red Lines

- No address-only login.
- No wallet-address-derived encryption keys.
- No arbitrary signing.
- No app-visible wallet RPC or node RPC.
- No private keys outside wallet-provider authority.
- No Essentials or UniversalX requirement for default Home unlock.
- No blockchain UI before fail-closed tests and capability schema exist.
