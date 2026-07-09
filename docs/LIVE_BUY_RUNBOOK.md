# Live DRM buy — operator runbook (the live-money last mile)

This is the **operator-driven** procedure for exercising a real on-chain DRM buy end to end
(Grant → Act → Prove) against a live EVM testnet. It is deliberately NOT part of the CI gate: the
gate injects `DrmResolver`/`DrmSettler`/`DrmConfirmer` mocks and never touches a network or a funded
wallet (a sandbox has neither). Everything the gate *can* prove about this path — the money
direction, the price gate, confirmation-aware settlement, the rail discriminator, the deadline
discipline — is already proven by the ratchets named in `KNOWN_GAPS.md` (MKT-DRM). This runbook is
the last mile the gate cannot run for you: a real dollar (of testnet value) moving under a mandate.

> **Safety.** Run this on a **testnet** (e.g. Base Sepolia) with a **throwaway funded wallet** first.
> The managed-signing path (`ELASTOS_DDRM_BUY_SIGN=wallet`) is a `dev-modes` build only — a release
> build never self-signs; it hands back the unsigned tx for an external wallet (SCOPE.md hard-gate).

## 0. Prerequisites

- A `dev-modes` build of the gateway: `cargo build -p elastos-server --features dev-modes`.
- Built capsules on `PATH` (or pointed at by env below):
  - `cargo build --manifest-path capsules/chain-provider/Cargo.toml`
  - `cargo build --manifest-path capsules/wallet-provider/Cargo.toml`
- A funded testnet account for the **managed** wallet (its address is minted on first use — see step 2;
  fund THAT address), and a deployed ACCESS_TOKEN listing (operative + tokenId + an active seller).

## 1. Environment (the money contract)

| Variable | Meaning |
|---|---|
| `ELASTOS_DDRM_RIGHTS=chain` | The LIVE rights + buy path (not `dev`/`chain-mock`). |
| `ELASTOS_DDRM_BUY_SIGN=wallet` | Managed-account signing (dev-modes only). Omit ⇒ unsigned-tx-for-external-wallet. |
| `ELASTOS_CHAIN_PROVIDER_BIN` | The real chain-provider binary (RPC-backed). |
| `ELASTOS_WALLET_PROVIDER_BIN` | The wallet-provider binary (holds the secp256k1 key; it NEVER leaves). |
| `ELASTOS_DDRM_WALLET_BASE` | Stable managed-key store dir (so the account survives across buys). |
| `ELASTOS_DDRM_CHAIN_ID` | The EVM chain id (e.g. `84532` for Base Sepolia). |
| `ELASTOS_DDRM_BUY_LEDGER` | The asset's channel/ledger (resolves KID→tokenId + the `buyAccess` `ledger` arg). |
| `ELASTOS_DRM_SPEND_UNIT` | meter-unit⇄pay-token mapping — the cap is a LITERAL on-chain ceiling (fail-closed: the live rail refuses to wire without it). |
| `ELASTOS_DRM_PAY_TOKEN` | The required pay-token; a listing in any other token is refused (abort-on-drift). |
| `ELASTOS_DRM_MIN_CONFIRMATIONS` | Depth floor before a buy promotes to charged (default 3). |
| `ELASTOS_DRM_RECONCILE_INTERVAL_SECS` | Arms the in-runtime confirmation scheduler (unset ⇒ manual reconcile). |
| `ELASTOS_CHAIN_READ_DEADLINE_SECS` | Per chain-conversation deadline (default 30; set above your P99 RPC roundtrip). |

## 2. Grant → Act → Prove

1. **Mint the managed account & fund it.** Start the gateway with the env above; the managed
   account address is minted deterministically on first wallet use. Fund that address with testnet
   gas + the pay-token, and confirm the ACCESS_TOKEN listing is active for it.
2. **Grant** a pay-mandate scoped to the asset with a cap that covers `price × 1` (quantity is pinned
   to 1 — a buy is one ACCESS_TOKEN): use the Mandates shell app (or `mandate_cmd grant`).
3. **Act.** Have the agent dispatch a signed `runtime.pay` intent bound to that mandate (or drive the
   `market-demo` quote-then-buy). The buy path: quote the on-chain price (read-only, refuses if the
   cap can't cover it) → durably custody the reservation `Pending` on the ledger (now tagged
   `PaymentRail::Drm`, Sprint 44) → sign in the wallet capsule → broadcast → return `Indeterminate`
   with the `drm:tx=<hash>;…` rail_ref. The reservation is HELD, never charged at broadcast.
4. **Confirm.** The scheduler (or a manual `reconcile`) reads the receipt: mined + `status==0x1` +
   ≥ `ELASTOS_DRM_MIN_CONFIRMATIONS` deep ⇒ promote to charged and bind the receipt's token-keyed
   `CapabilityUse.rail_ref`; reverted ⇒ refund exactly once; not-yet-mined/unreadable ⇒ stay Pending.
5. **Prove.** Export the `MandateReceipt` and verify it OFF the box:
   `elastos verify-receipt <receipt.json>` — it re-checks the signed chain and shows the bound
   `rail_ref` (`drm:tx=…;price=…;tok=…`). This is the portable proof a third party trusts.

## 3. What each failure means (the money direction, by construction — Sprint 43)

- **Refused before broadcast** (wallet-not-linked, sold out, price/pay-token drift, sign-leg timeout,
  a missing signature): `BuyError::PreBroadcast` ⇒ **refund** — the tx was provably never sent.
- **Failed at/after broadcast** (send-leg RPC error/timeout, a post-broadcast bookkeeping failure):
  `BuyError::Indeterminate` ⇒ **hold** the reservation; reconciliation resolves it from the chain.
  A hostile provider's error text can NOT flip this — the variant is the code path, not the string.

## 4. Wiring it into CI later (the network gate)

When a funded testnet + secret store is available in CI, gate a real-buy integration test behind a
`live-chain` cargo feature (or a `#[ignore]` run invoked explicitly) that reads the env above from CI
secrets and asserts: quote ≤ cap, a Pending record tagged `Drm` appears, the scheduler promotes it
after the depth floor, and the exported receipt verifies. Keep it OUT of the default gate — a test
that spends real value or needs a network must never run on every push.
