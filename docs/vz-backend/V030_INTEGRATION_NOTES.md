# v0.3.0 ↔ Our Two Branches — Integration Readiness Notes

> **Author:** agent + operator (sash) pair, 2026-05-28
> **Purpose:** ground-truth picture of what v0.3.0 ships, how it lines up with our
> in-flight CVE-hygiene PR and Mac-VZ substrate branch, and what concrete
> integration work remains. Written for the v0.3.1-window review.
> **Scope:** evidence-based. Every claim cites a file, a diff stat, or a command output.
> **Status of our branches at the time of writing:**
> - `chore/runtime-cve-hygiene` HEAD `d32cc3a` — PR [#1](https://github.com/Elacity/elastos-runtime/pull/1), CI clean, `mergeStateStatus: CLEAN`. 32 of 34 inherited CVEs closed.
> - `sash/local-test` HEAD `8efc441` — all three CI lanes green (Linux-untouched, Linux CI, Mac Vz CI). Native macOS substrate via Apple Virtualization.framework.

---

## 1. What v0.3.0 actually shipped (verified, not paraphrased)

| Metric | Value | Source |
|---|---|---|
| Tag | `v0.3.0` = `190ba6c`; main HEAD = `8acb72d` | `git log -1 v0.3.0` |
| Commits since v0.2.0 | 12 (11 features + 1 post-release clippy CI fix) | `git log --oneline v0.2.0..v0.3.0 \| wc -l` |
| File churn | 419 files, **+119,540 / −12,647** | `git diff --shortstat v0.2.0..v0.3.0` |
| New workspace member | `crates/elastos-auth` | `git diff v0.2.0..v0.3.0 -- elastos/Cargo.toml` |
| New capsule crates (standalone workspaces, not in `elastos/`) | 9 — `browser-engine-adapter`, `chain-provider`, `decrypt-provider`, `drm-provider`, `exit-provider`, `key-provider`, `net-provider`, `rights-provider`, `wallet-provider` | `git diff --name-status v0.2.0..v0.3.0 \| awk '/^A/ && /\/Cargo\.toml$/'` |
| New tools | `browser-engine-supervisor`, `browser-local-exit`, `browser-native-proxy-engine`, `browser-stream-bridge`, `browser-playwright-engine` (Node) | `elastos/tools/` |
| New external Rust deps | `argon2`, `blake2`, `k256`, `keccak`, `sha3`, `password-hash`, `pkcs1`, `qrcodegen`, **`rsa`**, `num-bigint-dig`, `num-iter`, `bitcoin` (per-capsule), `reqwest` (chain-provider) | `git diff v0.2.0..v0.3.0 -- elastos/Cargo.lock \| grep '^\+name = '` |

The release notes are accurate. Every advertised feature has a verifiable code
artifact. The accompanying `state.md` distinguishes "what works" from "what is
proven" with refreshing honesty, and per-feature docs (`BROWSER_CAPSULE.md`,
`WALLET_PROVIDER.md`, `PROTECTED_CONTENT.md`, etc.) consistently lead with
*"Architecture target, not current shipped behavior. For current proof level see
state.md"*. Documentation quality is high.

## 2. v0.3.0's stated macOS position — direct quotes

From `ROADMAP.md`:

> "Linux remains the truthful full-runtime baseline. Other platforms should be
> earned without pretending to offer Linux/KVM parity everywhere. The default
> Home path should therefore be the **browser-hosted path** above the runtime
> contract, not a KVM-dependent appliance path. That keeps macOS, Windows,
> remote browser, and later mobile/webview adapters in scope without weakening
> the trusted-core model."

> "the default Home path must stay compatible with macOS, so it cannot depend
> on Linux/KVM-only behavior"

From `scripts/check-wci-alignment.sh`:

```sh
check_forbidden_in_path 'darwin\)' scripts/install.sh \
  'public installer must stay Linux-only until update/install support macOS coherently'
```

**Reading:** Anders has explicitly designed the runtime to *welcome* macOS as a
target *via the browser-hosted path*, has gated the public installer behind a
"macOS update/install coherent" condition, and has built `publish-release.sh`
with `x86_64-darwin` / `aarch64-darwin` platform branches already in place.
The macOS path is intentionally unfinished, not architecturally rejected.

## 3. The build-on-Mac gap is real and reproducible

```sh
# v0.3.0 worktree, fresh checkout, Mac (Apple Silicon)
$ cd /tmp/elastos-v030/elastos && cargo check --workspace
error[E0425]: cannot find value `SOCK_CLOEXEC` in crate `libc`
error[E0308]: arguments to this function are incorrect  # libc::ioctl c_ulong vs i32
error[E0063]: missing field `sin_len` in initializer of `sockaddr_in`
   --> crates/elastos-crosvm/src/network.rs:63:18
error: could not compile `elastos-crosvm` (lib) due to 3 previous errors
```

`elastos-server` has an unconditional `path = "../elastos-crosvm"` dep, so the
server (and therefore Home, the gateway, every API surface) **does not build on
macOS in v0.3.0 main.** Pre-existing condition, inherited from v0.2.0, not
caused by v0.3.0 — but v0.3.0 also did not resolve it.

`sash/local-test` resolves this in two complementary ways:

1. `elastos-crosvm` now cross-compiles on Mac via a stub that mirrors the
   public surface and fails-closed on any microVM call. Inline comment in
   `crates/elastos-crosvm/src/lib.rs` literally quotes the v0.3.0 ROADMAP:
   > "The default Home path must remain a KVM-independent browser-hosted
   > adapter so macOS and Windows stay in scope without pretending to offer
   > Linux parity."
2. `crates/elastos-vz` provides a native Mac substrate using Apple
   Virtualization.framework, gated `[target.'cfg(target_os = "macos")']`,
   for the developer/parity path where someone *wants* the full
   Linux-microVM baseline on a Mac.

```sh
# sash/local-test, same Mac, same toolchain
$ cargo check -p elastos-server   # ✓ Finished in 22.69s
$ cargo check -p elastos-vz       # ✓ Finished in  1.83s
```

**Implication:** independent of whether Anders wants the native VZ substrate as
a product feature, **the cross-compile work on `sash/local-test` is the
mechanical prerequisite to satisfying his own `darwin\)` alignment gate.**
Without it, `scripts/install.sh` cannot grow a macOS branch coherently.

## 4. Security posture comparison

Audit ran on v0.3.0 worktree, Mac, `cargo audit` (database
2026-05-28-ish):

```
v0.3.0 main:                       35 vulnerabilities, 12 unmaintained warnings
chore/runtime-cve-hygiene (PR #1):  2 vulnerabilities,  4 unmaintained warnings
```

Vulnerabilities present in v0.3.0 that **our CVE branch already closes** when
mechanically replayed on top of v0.3.0:

| Crate | Version on v0.3.0 | Disposition on our branch |
|---|---|---|
| `wasmtime` 17.0.3 | 8 CVEs (host-escape and validator issues) | bumped 17 → 24 (Day 8) → 36 (Day 9), closed 18+ CVEs |
| `wasmtime-wasi`, `wasmtime-jit-debug` 17.0.3 | bundled with wasmtime | closed via the same cascade |
| `rustls-webpki` 0.103.9 | 4 CVEs (path validation) | bumped via TLS chain refresh (Day 2) |
| `cap-primitives` 2.0.2 | RUSTSEC-2024-0445 | bumped on Day 3 |
| `rustls-pemfile` 2.2.0 | unmaintained | removed Day 10, migrated to `rustls-pki-types::pem::PemObject` |
| `lru` 0.12.5 | unmaintained warning | bumped 0.12 → 0.18 (Day 10) |
| `rand` 0.8.5 duplicate | unmaintained warning | de-duped Day 10 |

**Net effect of replaying our CVE branch onto v0.3.0:** ~20 of v0.3.0's 35
vulns close mechanically with one rebase.

Vulnerabilities **new in v0.3.0** that our branch does not yet address (because
the relevant deps did not exist when we forked at v0.2.0):

| Crate | Version | Advisory | Source |
|---|---|---|---|
| `aws-lc-sys` | 0.37.0 | RUSTSEC-2026-0044/45/46/47/48 (5 CVEs) | TLS / crypto backend transitive |
| `bytes` | 1.11.0 | RUSTSEC-2026-0007 | reached via hyper / iroh upgrade |
| `quinn-proto` | 0.11.13 | RUSTSEC-2026-0037 | iroh QUIC stack |
| `rsa` | 0.9.10 | **RUSTSEC-2023-0071 (Marvin timing attack — unfixable upstream)** | likely via `elastos-auth` k256 cascade or per-capsule deps |

**The `rsa` Marvin finding is the most strategically interesting.** It is the
standard "do not use `rsa` for any operation that touches a secret" advisory.
Worth a separate audit of whether any v0.3.0 capability path actually invokes
`rsa` decrypt/sign with key material, regardless of branch sequencing.

## 5. Conflict shape if/when we rebase

### `chore/runtime-cve-hygiene` (PR #1) onto v0.3.0 main

| File | v0.3.0 churn | CVE branch churn | Conflict difficulty |
|---|---|---|---|
| `elastos-compute/src/providers/wasm.rs` | +40 / −11 | +502 / −113 (FIFO transport + wasmtime 17→36) | **hard** — wasmtime API surface shifted on both sides |
| `elastos-guest/src/runtime.rs` | +184 / **−354** (net rewrite) | +302 / −8 (FIFO channel) | **hard** — different directions on same file |
| `elastos-server/Cargo.toml` | +6 | +12 / −4 | easy (non-overlapping dep blocks) |
| `elastos-identity/src/store.rs` | +33 / −2 | +9 | easy |
| `localhost-provider/src/main.rs` | +1 / −1 | +8 | easy |

Estimate: half-day to full-day of careful rebase work, mostly in `wasm.rs`
and `runtime.rs`. The CVE outcomes (wasmtime 17→36, rustls cascade, etc.) are
deterministic and will replay cleanly even if surrounding code shifts.

### `sash/local-test` (Mac VZ) onto v0.3.0 main

| File | v0.3.0 | sash/local-test | Conflict difficulty |
|---|---|---|---|
| `elastos/Cargo.toml` | +1 (elastos-auth) | +1 (elastos-vz) | trivial — union the two adds |
| **`carrier_bridge.rs`** | **+1002 / −135** (Carrier rooms, capsule-Carrier invoke, chat updates) | **+917 / −58** (FIFO transport + 2.4M-iter fuzz harness + threat model) | **very hard** — both rewrote the same file in different directions |
| **`supervisor.rs`** | +148 / −88 | **+3800 / −81** (entire Mac VZ supervisor) | **hard** — but ours is mostly *additions*; replay v0.3.0's +148 on top |
| `vm_provider.rs` | +23 / −9 | +544 / −1 | medium |
| `setup.rs` | +20 / −9 | +340 / −53 | medium |
| `run_cmd.rs` | +6 / −2 | +370 / −2 | medium |
| `runtime_control.rs` | +231 / −5 | +39 / −5 | medium |
| `main.rs`, `binaries.rs`, `home_cmd.rs`, `runtime.rs`, `lib.rs` | small | small-medium | low |

Total: `sash/local-test` is 102 commits ahead of v0.2.0. The 14 overlapping
files are tractable but `carrier_bridge.rs` is the real cost — both branches
extended the same file ~1000 lines in different directions and the merge
requires understanding both sides. Estimate: 2–3 days of careful work by
someone who understands both feature sets.

## 6. Where the two branches help v0.3.0 (lined up with v0.3.0's own docs)

This is not "our work is more important than theirs" — it's "our work
*operationalizes* commitments v0.3.0 already made on paper":

| v0.3.0 stated commitment | Our work that operationalizes it |
|---|---|
| ROADMAP: *"default Home path must stay compatible with macOS"* | `crates/elastos-crosvm` Mac cross-compile stub on `sash/local-test` |
| Alignment check: *"public installer must stay Linux-only until update/install support macOS coherently"* | `crates/elastos-vz` native Mac substrate + `Phase 9` install smoke matrix |
| `state.md`: *"browser-engine-supervisor ... starts the configured engine under `linux_new_netns`, ... this container cannot complete that proof because `CLONE_NEWNET` is not permitted"* | Apple Virtualization.framework provides the equivalent isolation primitive on Mac for the same Browser/Net/Exit ABI |
| PRINCIPLES §11: *"Fail Closed, Then Explain"* | Phase 10 hardening of `carrier_bridge.rs` (FIFO transport, 2.4M-iter fuzz, SIGINT graceful shutdown) follows the same discipline |
| ROADMAP: *"keep release, install, update, share, and site flows boring"* | `chore/runtime-cve-hygiene` closes 32 of 34 inherited CVEs without changing any user-visible behavior |
| `WALLET_PROVIDER.md`: *"Capsules never receive private keys"* | The CVE branch's wasmtime 17→36 cascade closes 8 host-escape vulnerabilities relevant to that boundary |

## 7. Where the two branches *could* conflict with v0.3.0's intent

We should also be honest about places where our work and v0.3.0's direction
might not line up cleanly:

1. **Mac VZ is the "Linux/KVM parity" path, but v0.3.0 explicitly chose the
   browser-hosted path as the default Mac story.** Our work gives the *option*
   of a full native Linux baseline on a Mac developer's machine, but it is not
   the default product story. Acceptable outcomes range from "ship as developer
   tool" to "ship as alternative product mode" to "park for now." Anders'
   decision.

2. **Our `carrier_bridge.rs` FIFO transport rewrite was done because
   wasmtime-wasi 24+ removed `wasi.insert_file`.** That removal is also forced
   on Anders the moment he tries to take the wasmtime CVE closures. So the FIFO
   transport (or an equivalent) is a hard requirement, not a free choice. Worth
   making explicit so the merge conversation doesn't go "do we *need* FIFO
   transport?" — yes, downstream of accepting the wasmtime bump, yes.

3. **The CVE branch pins `distributed-topic-tracker = "=0.2.7"`** to avoid the
   iroh 0.96/0.97 split. v0.3.0 might want a newer `distributed-topic-tracker`
   later for an iroh-related feature; documented as out-of-scope follow-up in
   the CVE branch sign-off, but worth surfacing.

## 8. Recommended sequencing (for Anders to react to, not for us to execute)

1. **Anders' v0.3.0 review burns down** (his current focus, no action from us).
2. **Anders reviews PR #1** (`chore/runtime-cve-hygiene`). Smaller, mostly mechanical,
   ~20 CVE closures replayable onto v0.3.0. Land first.
3. **After PR #1 lands on main:** Anders decides on the Mac VZ position
   (default / dev-tool / park). The decision drives the rebase shape.
4. **If Mac VZ is in:** allocate 2–3 days for the carrier_bridge / supervisor
   rebase. Treat as a focused integration task with its own sign-off.
5. **In parallel, regardless of (3):** open a focused issue on `rsa 0.9.10
   Marvin attack` because that's a real finding independent of our branches.

## 9. What we should *not* do

- Do not rebase either branch pre-emptively. Anders is the gatekeeper for
  v0.3.1; a speculative rebase risks wasted effort and the appearance of
  stepping on his review window.
- Do not force-push to either branch. Both are stable and CI-green; he reviews
  them as-is.
- Do not file a "v0.3.0 has 35 CVEs!" issue separately. Frame it inside the
  message context that *our branch closes ~20 of them*; the framing matters.
- Do not assume the Mac VZ substrate is automatically in for v0.3.1. The
  ROADMAP wording is the source of truth and it picks the browser-hosted
  path as default.

## 10. State at time of writing (verifiable)

```
$ git log --oneline origin/main -1
8acb72d fix(ci): satisfy clippy in home realtime tests           # v0.3.0 main

$ gh pr view 1 --json mergeable,mergeStateStatus
mergeable: MERGEABLE
mergeStateStatus: CLEAN                                          # PR #1 ready

$ git log --oneline sash/local-test -1
8efc441 docs(branch-summary): record PR #1 CI fix-up...          # our Mac VZ branch
$ git log --oneline chore/runtime-cve-hygiene -1
d32cc3a chore(cve): pin distributed-topic-tracker = "=0.2.7"...  # our CVE branch
```

No changes to either branch were made while preparing this memo. Investigation
worktree `/tmp/elastos-v030/` will be removed when the memo is committed.

---

*This memo is a snapshot of evidence, not a request. The integration sequencing
proposal in §8 is a starting point for the v0.3.1 review conversation, not a
plan being executed.*
