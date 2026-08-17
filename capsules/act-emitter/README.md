# act-emitter — spend/audit VERIFICATION FIXTURE (not a shipped product capsule)

This capsule exists **only** to verify the microVM spend-meter and durable-audit
path on real hardware. It is intentionally tiny, non-interactive, and
deterministic. Do **not** ship it, list it in a product catalog, or treat it as
an example app — it is a test instrument.

## What it does

On boot it reads `count` from the launch config (`elastos.command` →
`ELASTOS_COMMAND_B64`, default `7`), acquires a write capability, then performs
**exactly `count`** `carrier_invoke` storage writes via the *shipped* guest
carrier client (`elastos_guest::runtime::RuntimeClient` — the same
`carrier_invoke` path `capsules/chat/src/carrier.rs` uses). It prints a
machine-greppable trace to the guest console:

```
ACT_EMITTER_START count=<N>
ACT_EMITTER_CAP ok | ERR <msg>
ACT <i> ok | ACT <i> REFUSED budget_exhausted | ACT <i> ERR <msg>
ACT_EMITTER_DONE ok=<n> exhausted=<n> other_err=<n>
```

So with `ELASTOS_DEFAULT_SPEND_BUDGET=N` and `count=N+1`, the run yields exactly
`N` successful debits (`spent=N`, `remaining=0`) and a `budget_exhausted`
refusal on the `N+1`-th act — counted, reproducible evidence.

## Storage root: `localhost://Public/ActEmitter/*` (deliberate)

The fixture targets the `Public/` root, **not** `Users/self`. `Public` is a
plaintext, file-backed root that the bridge's `scope_current_user_alias` passes
through without a principal — so the fixture exercises the metered carrier path
under the plain `elastos capsule` CLI, which cannot inject a signed Home
launch-grant (that is the production identity flow).

The spend meter / carrier / audit code is **byte-identical regardless of storage
root** (the debit is `CARRIER_ACT_COST` per `carrier_invoke`, charged before
dispatch; the `SpendDebit`/`BudgetExhausted` records are the same), so the
verification is valid. The one honest residual: `Users/self`-scoped storage
metering is still unverified on hardware — see `docs/KNOWN_GAPS.md` (G-HWV).

## How to run (real nested-KVM box)

Prereqs: a box prepared per `docs/MICROVM_LOCAL_KVM_PROVISIONING.md` (a kernel
that actually boots crosvm + the AppArmor sysctl + the offline catalog).

```bash
# 1. build the rootfs (musl-static binary + busybox + vsock-proxy → ext4)
bash scripts/build/build-rootfs.sh act-emitter --output <artifacts>

# 2. stage into the data-dir catalog (capsules/act-emitter/ + components.json entry)

# 3. start serve with durable audit + a budget
ELASTOS_AUDIT_LOG_PATH=<data_dir>/audit/custody.log \
ELASTOS_DEFAULT_SPEND_BUDGET=5 \
elastos serve

# 4. launch through the supervisor (the metered path)
elastos capsule act-emitter --config '{"count":6}'
# → ACT 1..5 ok ; ACT 6 REFUSED budget_exhausted ; ok=5 exhausted=1
```

First verified 7/7 on real nested-KVM (`flint @ 5d4f4c7d1`, 2026-06-29). The
build is self-contained (`[workspace]` in `Cargo.toml`); it is not part of the
main Rust workspace.
