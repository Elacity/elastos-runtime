# Wallet Provider

`wallet-provider` is the blockchain-quadrant authority boundary for wallet
proofs, account links, approvals, typed signatures, and transaction requests.
It is not the Runtime root of identity and it is not an app SDK.

The contract is:

`signed launch-token v4 -> verified RuntimeWalletAuthority -> private RuntimeWalletAdapter -> WalletProviderRequestV2 (Wallet Bus 2.3) -> wallet-provider`

Capsules never receive private keys, browser wallet objects, raw wallet RPC,
node RPC, seed phrases, or provider SDK handles. A wallet address is a proof
binding on a Runtime principal, not the principal itself.

The generic provider plane exposes only read-only
`elastos://wallet/meta/status`. Generic HTTP, component/Carrier, attached
component, capability-request, and Inspect callers cannot select a principal,
forward a local bearer token, dispatch `wallet_contract`, or request any Wallet
account, proof, approval, signing, secret, Recovery, or transaction operation.

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
finish the request. Opaque connector frames initiate the fixed link or approval
intent, while the trusted Home host owns only the top-level injected-provider
effect; System does not initiate wallet-link ceremonies.

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
tools remain connector methods. The connector frames remain opaque and never
receive browser-extension provider objects. For injected MetaMask/Brave and
UniSat, a closed connector intent asks the trusted top-level Home host to perform
the exact Runtime challenge or typed approval effect. WalletConnect retains its
configured connector-owned adapter path.

## Storage

Wallet state belongs in runtime-managed provider storage, not browser local
storage and not app capsule storage:

`localhost://ElastOS/SystemServices/Wallet/...`

The current implementation stores linked-account metadata, default-account
preferences, encrypted managed EVM wallet envelopes, pending SIWE proof
challenges, and typed wallet approval requests in `wallet-state.json`. Account
records and defaults are keyed by `principal_id`; proof challenges are
short-lived, single-use, and bind the exact challenge resource.
The typed `RequestApproval` operation records a pending approval for an allowed
intent and returns no signature. Runtime resolves and supplies the exact
principal-owned account, chain namespace, intent, resource, reason, payload, and
expiry before invoking the private adapter.

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

The public provider surface is intentionally limited to one operation:

| Operation | Capability resource | Action | Purpose |
|-----------|---------------------|--------|---------|
| `status` | `elastos://wallet/meta/status` | `read` | Report bounded provider identity, version, and adapter status without principal data |

All principal-sensitive work uses the private Runtime-local Wallet Bus 2.3
envelope. Runtime derives `RuntimeWalletAuthority` only from a successfully
validated signed launch token, constructs `WalletProviderRequestV2`, and
dispatches a typed `WalletProviderOperationV2` through
`RuntimeWalletAdapter`. The typed variants cover account reads and writes,
proof challenges and verification, approval lifecycle operations, validated
chain-outcome projection, and managed Recovery import/export. They are not
generic provider methods or capsule capability resources.

Generic `wallet_contract`, legacy proof/account/approval/signing/secret/Recovery
names, signing URI derivation, and Wallet transaction prepare/broadcast all fail
before provider invocation. Transaction prepare and broadcast remain typed
`chain-provider` effects; wallet-provider owns proof bindings, approval state,
managed signing, and connector completion receipts.

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
wallets are handled by dedicated opaque connector capsules plus the trusted Home
host. MetaMask/Brave and UniSat frames can read their Runtime account and
approval summaries, but their only effect request is the closed
`home:wallet-connector-effect` contract. Runtime's Home connector endpoints
validate exact same-origin Home authority and a carried connector launch token
for the same principal, session, proof, and grant before issuing a challenge or
typed handoff. Home alone calls the top-level injected provider and returns only
status to the connector frame. The generic direct connector completion path
remains for configured WalletConnect, and only explicitly allowlisted
connectors can use it. Wallet and Inbox can reject pending requests.
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

External Bitcoin proof requests use the same approval contract. For UniSat,
Runtime returns the exact typed Bitcoin proof handoff only to the trusted Home
bridge; Home asks the top-level provider to sign that message and completes the
approval only after wallet-provider verifies the signature against the linked
address, selected proof type, and stored Runtime challenge. Neither Wallet nor
the opaque connector frame handles a free-form message or returned signature.

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

`passkey unlock -> open opaque approval-method connector capsule -> send one closed link intent to trusted Home -> Home discovers exact MetaMask or Brave fallback -> Runtime validates matching Home + connector launch-token v4 authority -> Runtime issues SIWE challenge -> Home signs only that challenge -> wallet-provider verifies and links account + connector_id to the existing principal -> audit -> revoke -> prove replay/expiry/wrong-chain/wrong-origin/wrong-connector fail closed`

The built-in Bitcoin proof path is now:

`passkey unlock -> create built-in managed Bitcoin wallet -> Runtime issues BIP-322 challenge -> app requests typed bitcoin_bip322_proof -> Wallet/Inbox review -> fresh passkey approval in Wallet -> wallet-provider signs inside its boundary -> signed audit/receipt`

The optional external Bitcoin proof path uses the same connector boundary:

`passkey unlock -> open opaque UniSat connector -> send one closed link intent to trusted Home -> Runtime validates matching Home + connector launch-token v4 authority -> Runtime issues the address-selected Bitcoin proof challenge -> Home asks top-level UniSat to sign that exact challenge using the selected proof mode -> verify through wallet-provider -> link BTC proof binding to the existing principal -> audit -> revoke`

For approval-gated Bitcoin proof signing after a BTC address is linked:

`app requests typed bitcoin_bip322_proof -> Wallet/Inbox review -> opaque UniSat connector requests exact approval id -> Runtime gives trusted Home the typed handoff -> top-level UniSat signs the exact Runtime proof message using the selected proof mode -> wallet-provider verifies and records a signed receipt`

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
capsules do not use WalletConnect or generic Wallet provider calls; their
product routes enter the private typed Wallet Bus through verified Runtime
authority.

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
documents `requestAccounts` and `signMessage`. Managed Bitcoin accounts remain
Bitcoin-mainnet native P2WPKH and managed signing remains
`managed_btc_p2wpkh`. Separately, wallet-provider source tests cover external
BIP-322 simple verification for native P2WPKH and Taproot P2TR, plus legacy
Bitcoin signed-message verification for P2PKH and nested SegWit P2SH-P2WPKH.
Those verifier tests are not real UniSat compatibility evidence. Product
acceptance still requires pinned real-wallet evidence for each claimed path,
and the weaker proof-strength policy for legacy signed-message verification
remains open.

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
`/api/auth/btc/challenge` and `/api/auth/btc/verify` routes remain
connector-token scoped for direct configured connector use; System/Home tokens
cannot create or verify BTC wallet links there. Injected UniSat instead uses the
dedicated Home connector endpoints, which require both exact same-origin Home
authority and a carried UniSat token bound to the same principal, session,
proof, and grant. Internally,
`bitcoin_challenge` issues a short-lived, single-use challenge for a Bitcoin
address, and `verify_bip322_proof` consumes it only when the signed message
matches the stored challenge exactly. The external verifier has source-tested
support for BIP-322 simple native P2WPKH and P2TR proofs under
`bip322_simple`, and for Bitcoin signed-message P2PKH and P2SH-P2WPKH proofs
under `bitcoin_signed_message`. The built-in managed Bitcoin account is
different: it remains native P2WPKH and uses `managed_btc_p2wpkh` when signing
the Runtime-bound proof after approval. Unsupported networks and scripts,
malformed witnesses, replayed or expired challenges, wrong-message signatures,
and non-Runtime managed proof messages fail closed.

Source-tested verification is not a product compatibility or capability-strength
claim. Real UniSat evidence for P2WPKH, P2TR, P2PKH, and P2SH-P2WPKH remains
open, as does an explicit weaker-proof policy for `bitcoin_signed_message`.
P2WSH, multisig, hardware-wallet-specific behavior, and any additional script
types still need pinned vectors, wallet evidence, connector UX, and policy
before they can be claimed as supported.

## Red Lines

- No address-only login.
- No wallet-address-derived encryption keys.
- No arbitrary signing.
- No app-visible wallet RPC or node RPC.
- No private keys outside wallet-provider authority.
- No Essentials or UniversalX requirement for default Home unlock.
- No blockchain UI before fail-closed tests and capability schema exist.
