# Phase 9 Day 4 — Bootstrap auto-re-sign on missing entitlements

> **Outcome (2026-05-26):** Closed the silent-corruption footgun
> the Day-3 work surfaced: every `cargo build -p elastos-server`
> invalidates codesign and silently drops the four Vz + JIT
> entitlements baked into our dev-sign plist, leaving the
> operator with a runtime that **looks** healthy
> (`elastos home --status` still works because that command
> doesn't need Vz or JIT) but **fails closed** on the two
> end-to-end smokes that matter — `elastos run capsules/home`
> SIGKILLs the first time wasmtime touches a JIT page, and
> `elastos run ubuntu-base` refuses to boot. Day 4 adds a
> single block to `scripts/dev/mac-local-setup.sh` that detects
> the missing `com.apple.security.virtualization` entitlement
> in the codesign XML and invokes the existing
> `sign-elastos-vz/sign.sh` automatically. Idempotent: a
> correctly-signed binary triggers exactly zero work.
>
> **Anchor:** [`PHASE_9_DAY_3_NOTES.md`](PHASE_9_DAY_3_NOTES.md)
> § 4 called this out as the Day-4 candidate.

## 1. The silent failure mode

After any `cargo build -p elastos-server` the dev binary
transitions from "signed with four entitlements" back to plain
adhoc-linker-signed:

```text
# Right after Phase 8 Day 8:
CodeDirectory v=20500 size=… flags=0x10002(adhoc,runtime) …
Entitlements: com.apple.security.virtualization, …allow-jit, …

# After ANY `cargo build -p elastos-server` since:
CodeDirectory v=20400 size=… flags=0x20002(adhoc,linker-signed) …
Entitlements: (none)
```

The two failure modes this enables are:

1. **`elastos run capsules/home` exits 137 with no stderr.** macOS's
   Hardened Runtime SIGKILLs the wasmtime engine the first time it
   `mprotect(PROT_EXEC)`s a JIT page because the binary doesn't
   carry `com.apple.security.cs.allow-jit`. The kill happens before
   any output is flushed, so the operator sees nothing.

2. **`elastos run ubuntu-base` refuses to boot.** Vz's
   `validateWithError:` rejects the configuration the moment it
   notices `com.apple.security.virtualization` isn't present.
   This _does_ surface a clear error, but only because Vz fails
   open early — the silent JIT case (1) is the dangerous one.

Both failure modes are invisible to `elastos home --status` and
to `elastos home` (the dashboard launches just fine without Vz
or JIT — neither is needed by the managed-home runtime or the
HTTP API).

## 2. The fix

Three lines into `scripts/dev/mac-local-setup.sh`, between the
manifest write and the live `--status --json` verifier:

```bash
DEBUG_ELASTOS="$REPO_ROOT/elastos/target/debug/elastos"
SIGN_SCRIPT="$REPO_ROOT/scripts/dev/sign-elastos-vz/sign.sh"

if [[ -x "$DEBUG_ELASTOS" ]]; then
  if ! codesign -d --entitlements - --xml "$DEBUG_ELASTOS" 2>&1 \
        | grep -q "com.apple.security.virtualization"; then
    echo "[mac-local-setup] debug binary missing Vz/JIT entitlements — re-signing"
    "$SIGN_SCRIPT" "$DEBUG_ELASTOS" 2>&1 | sed 's/^/  /'
  fi
fi
```

We pick `com.apple.security.virtualization` as the sentinel
because all four entitlements live in the same plist
(`vz.entitlements.plist`); if one's absent, they're all absent,
so a single grep is sufficient.

`sign.sh` is already idempotent and ad-hoc-signing-only, so it
runs in <300 ms and writes a deterministic codesign blob — re-
running it has no observable effect besides relinking the
signature.

## 3. Smoke

### 3.1 Strip-then-bootstrap

```text
$ codesign --remove-signature elastos/target/debug/elastos
$ # entitlements: gone
$ scripts/dev/mac-local-setup.sh
…
[mac-local-setup] debug binary missing Vz/JIT entitlements — re-signing
    identity: - (ad-hoc; local development only)
    Verifying entitlements were applied...
    Done. `…/elastos/target/debug/elastos` can now drive Apple's Virtualization.framework.
[mac-local-setup] verifying via: elastos home --status --json
  services ready: 6 / 8
…
[mac-local-setup] OK
$ # entitlements: restored
```

### 3.2 Idempotent re-run

```text
$ scripts/dev/mac-local-setup.sh | grep -E 'building|re-signing|services'
[mac-local-setup] building provider: shell
…
  services ready: 6 / 8
```

No `re-signing` line — the sentinel grep finds
`com.apple.security.virtualization` in the XML and the block
short-circuits. Same result, same exit code, ~1 s wall clock.

### 3.3 Downstream smokes preserved

`elastos home --status --json` reports 6 / 8 (matches Day 3).
`elastos run capsules/home` (WASM standalone) prints the
launch banner and exits 0 (JIT working). The dev-signing plist
already shipped with both Vz + JIT entitlements after Phase 8
Day 8, so the auto-re-sign restores the exact baseline.

## 4. Why the auto-resign lives in the bootstrap script

Three places could have hosted the check:

| Location                              | Pros                                       | Cons                                                                                            |
| ------------------------------------- | ------------------------------------------ | ----------------------------------------------------------------------------------------------- |
| Inside the elastos binary (post-build hook) | Survives any build path, not just bootstrap | Cargo doesn't have a clean post-build hook; would need a `build.rs` that shells out to `codesign`. Adds Mac-only build-time complexity. |
| Cargo wrapper script                  | Catches every `cargo build`                | Asks every developer to use the wrapper. Easy to bypass.                                        |
| `mac-local-setup.sh`                  | Discoverable, already the "one command to make Mac work" path, idempotent | Only triggers when the operator runs the bootstrap (not when they re-run `cargo build` by hand). |

The bootstrap script wins because:

1. The Mac source-checkout flow is **always** "make a change →
   `cargo build` → re-bootstrap → smoke." The re-bootstrap step
   was already the natural place to re-validate everything
   else; signing fits the same shape.
2. Detection is two `codesign` calls — no shell magic, no
   build-system entanglement.
3. The hand-run `cargo build → cargo run` flow that bypasses
   the bootstrap will still surface the failure cleanly via the
   existing `sign.sh --verify-only` operator recipe in
   `docs/MAC.md`.

If a future operator does want fully automatic protection, a
~10-line Cargo wrapper at `scripts/dev/cargo` invoking the same
sign script would be the next step. Day-5+ candidate, no
substrate change.

## 5. Files touched

- `scripts/dev/mac-local-setup.sh` — +20 LOC (one `if`, one
  `codesign | grep` check, one sign-script invocation; +
  documentation comments).
- `docs/vz-backend/PHASE_6_PLAN.md` — status banner extended.
- `docs/vz-backend/PHASE_9_DAY_4_NOTES.md` — this file.

Zero substrate code touched. Zero new tests — the smoke script
is the test (strip + run + check).
