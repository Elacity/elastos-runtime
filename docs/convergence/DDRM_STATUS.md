# dDRM chain — status & review package

**Branch:** `feat/decrypt-provider-cenc` (based on `origin/0.4.0`)
**State:** the full Elacity dDRM provider chain is **fail-closed**, **compiles to
`wasm32-wasip1`**, **executes under WASI**, and has **verified inter-provider
contract handoffs**. The only thing between here and live decrypt is one
architecture decision (the CEK/ciphertext rail) — see `DDRM_DECRYPT_RAIL.md`.

## The chain

```
app/viewer --drm/open--> drm-provider --sequences--> rights -> key -> decrypt --scoped output--> app
                                          RightsReceipt -^   ReleaseReceipt -^ (wrapped CEK only)
```

## Parity table (proven bar)

| Provider | Role | Fail-closed | Host tests | wasm32-wasip1 | WASI smoke |
| --- | --- | --- | --- | --- | --- |
| `drm-provider` | orchestrator (`drm/open`) | yes | 12 | builds | 4/4 |
| `rights-provider` | rights decision | yes | 9 | builds | 4/4 |
| `key-provider` | key release (PQ-hybrid) | yes | 9 | builds | 4/4 |
| `decrypt-provider` | decrypt/render (+ cenc engine) | yes | 17 | builds | 4/4 |

## Security properties proven

- **Zero ambient authority surfaced.** Every provider's `status` advertises the
  raw authority it blocks (`raw_cek`, `chain_rpc`, `wallet_rpc`, `key_backend_sdk`,
  `kubo_api`, `elacity_sdk`, …) and wire-rejects hidden authority fields
  (`deny_unknown_fields`).
- **Fail-closed by default.** Every operation returns `not_configured` after full
  validation until its real backend exists. Invalid/mis-bound input returns
  `invalid_request`. Nothing opens by accident.
- **CEK containment.** The CEK only ever appears `wrapped` (key step) or
  contained + zeroized inside the cenc engine (decrypt step). The decrypt-step core
  seam is tested to leak neither the CEK nor plaintext to the caller.
- **Authorization binding.** `key-provider` verifies the upstream
  `RightsDecisionReceiptV1` (allowed + principal/session/object/right must match)
  before any release.
- **Contracts compose.** `drm-provider::chain_seam_tests` prove a
  `RightsDecisionReceiptV1` deserializes into the key request and a
  `ReleaseReceiptV1` into the decrypt request — shared-type drift fails loudly.

## How to run it yourself

```bash
# one-time prerequisites
rustup target add wasm32-wasip1
brew install wasmtime

# whole chain, one command:
scripts/ddrm-chain-smoke.sh

# per-provider host tests:
( cd capsules/drm-provider     && cargo test )
( cd capsules/rights-provider  && cargo test )
( cd capsules/key-provider     && cargo test )
( cd capsules/decrypt-provider && cargo test )
```

## The one open decision (for Anders)

How the CEK (VM-sealed) and ciphertext reach the decrypt boundary. Hybrid chosen
(decrypt step *receives* sealed material; upstream rights→key is a provider chain).
Full options, recommendation, and questions in `DDRM_DECRYPT_RAIL.md`.

## Isolation tier

Providers ship as **`wasm` now** (proven cross-platform, runs on macOS today);
**microVM** remains the later max-isolation upgrade from the same Rust source. The
fail-closed contract is tier-independent. Rationale in `DDRM_DECRYPT_RAIL.md`.

## Commits (on `feat/decrypt-provider-cenc`, not yet pushed — GitHub suspension)

dDRM-related, newest last:
- vendor PC2 cenc-decrypt engine as fail-closed backend
- decrypt-provider: tested decrypt-step core seam (Branch-by-Abstraction)
- decrypt-provider: WASI-sandbox smoke harness
- key-provider: rights-receipt binding + wasm/WASI bar
- rights-provider: WASI smoke (chain parity)
- drm-provider: WASI smoke + cross-provider contract-seam tests
- unified `scripts/ddrm-chain-smoke.sh` + this status doc

Plus docs: `DDRM_DECRYPT_RAIL.md`, `CONVERGENCE_PLAYBOOK.md`, `PRODUCT_VISION.md`.
