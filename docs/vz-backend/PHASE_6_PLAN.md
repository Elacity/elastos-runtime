# Phase 6 — Ship: Truthful darwin-arm64 + signed Mac binary + tagged release

> **Status:** **Phase 6 substrate CLOSED. Days 1–7 complete: a real Linux 5.15 kernel boots end-to-end through `elastos-vz` on a real Apple Silicon Mac, reaching userspace handover (`Run /init`) in ~0.3 s wall-clock — full kernel printk captured via our `vm_console` tracing forwarder (see [`PHASE_6_DAY_7_NOTES.md`](PHASE_6_DAY_7_NOTES.md) § 4). 14/14 elastos-vz tests green including `single_vm_boots_to_userspace` (Day 7) and `concurrent_load_with_real_kernel` (Day 6); no substrate code path remains untested. Build-recipe limitation banner + reproducible 5-command recipe shipped Days 6–7. Phase 7 CLOSED Days 1–6 (Mac substrate operator-ready: setup fetches Canonical-pinned kernel+initrd, supervisor wires all four Vz paths to the macOS data dir, `elastos doctor` inspects them cleanly with quiet UX — 386/386 elastos-server lib + 109/109 elastos-vz tests green; Linux byte-identical). Phase 8 STARTED Days 1–2: Day 1 decided the rootfs artifact strategy (Canonical's pinned `ubuntu-22.04-server-cloudimg-arm64.squashfs`, 411 MB, `release-20260515`) and audited that the Mac block-device launch pipeline is already fully wired (block FFI, Vz builder, `VmConfig.rootfs_path`, capsule-discovery glob — all present; substrate is complete, Phase 8 is distribution + integration). Day 2 wired the rootfs into the install plan: added `external.rootfs` to `components.json` with the SHA256 pinned from Canonical's signed `SHA256SUMS`, added the entry to the `minimal`/`chat`/`full` profiles, and smoke-tested end-to-end on this Mac — `elastos setup --profile minimal` now downloads → SHA256-verifies → installs the 411 MB squashfs at `~/Library/Application Support/elastos/capsules/ubuntu-base/rootfs.ext4` in ~55 s; `file` reports `Squashfs filesystem, version 4.0, xz compressed`. Zero code changed (the fetcher's `create_dir_all(parent)` already supports paths under `capsules/`); zero regressions (**386/386** lib tests). Day 3 extended `elastos doctor` to surface the rootfs as a first-class triage row between `initrd:` and `state_dir:` — reads the install path from `manifest.external.rootfs.install_path` joined to `data_dir`, reports `[present] size 411.0 MB` (or `[absent — run: elastos setup --profile minimal]` on an empty install), renders the manifest URL + SHA256 + size in `--verbose` mode, and explicitly suppresses the kernel-validator stanza (rootfs is not a kernel). +2 paired present/absent tests for symmetry with the vmlinux row, total **388/388** elastos-server lib tests passing. After Day 3 the triage tool answers the full "is the system ready to boot a guest?" question — kernel + initrd + rootfs all visible in one glance. Day 4 closed the gap from "artefacts staged" to "substrate actually exercised against them": three discover helpers in `concurrent_launch.rs` (kernel/initrd/rootfs) were hard-coding `~/.local/share/elastos/` (Linux-only path that doesn't exist on Mac), so every Mac run was silently skipping via `eprintln + return`. Refactored to use `dirs::data_dir().join("elastos")` (Linux byte-identical, macOS resolves to `~/Library/Application Support/elastos`), fixed a second latent bug (`discover_initrd` looked for the legacy filename `bin/initrd-generic` instead of the canonical `bin/initrd` Phase 7 Day 2 standardised on), and ran both Mac-only integration tests end-to-end on this Mac. `concurrent_load_with_real_kernel` **PASSES**: three VMs concurrently constructed `VzConfig` + `VmConfig` against the Day-2 kernel + rootfs, all three cleared `validateWithError:`, three distinct `CapsuleId`s minted under contention in 0.01s wall-clock. `single_vm_boots_to_userspace` **also PASSES**: the kernel-console tracing forwarder captured arm64 Linux printk reaching `Run /init` in ~131ms of kernel wall-clock — visible boot evidence that the same substrate boots a real Linux kernel + initramfs through to userspace handover inside Apple Vz. **3/3 elastos-vz integration tests on Mac, +388/388 elastos-server lib, +95/95 elastos-vz lib — zero regressions.** Day 5 closed the v0.1 demo bar: shipped a post-install hook in `setup.rs` that auto-writes a default `capsule.json` next to the rootfs (`ubuntu-base`, idempotent, +3 paired unit tests guarding schema drift), and extended `run_cmd.rs` with two-part wiring — (a) `resolve_capsule_by_name` rewrites `elastos run ubuntu-base` to `<data_dir>/capsules/ubuntu-base/` when the positional path doesn't exist, guarded against `..`/`/`-prefixed inputs, (b) a new `run_microvm_standalone()` lane that boots a standalone MicroVM in-process when no `elastos serve` daemon is running, mirroring the `vm-debug boot` pattern and exposing the same kernel-console tracing forwarder. Smoke-tested end-to-end on this Mac: first attempt (boot_args missing `root=`) cleanly surfaced Ubuntu's initramfs error `No root device specified. Boot arguments must include a root= parameter.` and panic-rebooted — exactly the real-substrate iteration the Day-5 prompt anticipated. Second attempt with `boot_args = "console=hvc0 reboot=k panic=1 root=/dev/vda rootfstype=squashfs ro init=/sbin/init"` boots **Ubuntu 22.04 LTS arm64 to systemd userspace** in ~3 seconds wall-clock: D-Bus, irqbalance, rsyslog, kmsg-save, Path/Socket/Basic-System targets all `[  OK  ]`. Expected `[FAILED]` on `systemd-logind` + a few other write-needing services because the squashfs is read-only — Day-6 writable-overlay scope. Also fixed a stale `rootfs.size` in `components.json` (430985216 → 431013888) that was causing every dev-iteration `elastos setup` to needlessly re-download. **391/391** elastos-server lib (+3 over Day 4), zero regressions. Day 6 closed the writable-rootfs gap: shipped a new `overlay_initrd.rs` module that builds a minimal newc-CPIO archive containing a single `/init` script and **appends it to Canonical's pristine `bin/initrd`** at `elastos setup` time (idempotent byte-compare). The Linux kernel handles concatenated initramfs archives natively (`init/initramfs.c`), so our `/init` shadows Ubuntu's — it mounts the squashfs at `/lower`, a tmpfs at `/upper` (256 MiB ephemeral), overlays them at `/newroot`, then `switch_root`s into the merged tree and execs `/sbin/init`. Three consumers (supervisor Mac branch, `run_cmd` standalone lane, `concurrent_launch.rs` discover_initrd) all go through a new shared `resolve_initrd_path()` helper that prefers `bin/initrd-overlay` when present, falls back to `bin/initrd` — no copy-pasted prefer-overlay branches. Smoke-tested end-to-end: first attempt panicked on `/dev/vda: Can't open blockdev` (real-substrate bug: virtio PCI probing is async, our /init raced the bus enumeration); fix was a 5s poll loop on `[ -b "$ROOT" ]` with 100ms granularity + a defensive fallback to `/sbin/init` from the initramfs if vda never appears. Second attempt: **Ubuntu 22.04.5 LTS boots cleanly to `ubuntu login:`** in ~6s wall-clock. 110 `[  OK  ]` services (D-Bus, irqbalance, rsyslog, sshd, systemd-logind, unattended-upgrades, pollinate, networkd-dispatcher, …), only 1 `[FAILED]` (the cosmetic `multipath-tools` SAN/iSCSI unit, irrelevant in any microVM). systemd-logind — the Day-5 headline failure — now starts cleanly. **`Reached target Login Prompts`** + **`ubuntu login:`** visible on the captured console. **403/403** elastos-server lib (+12 over Day 5: 9 CPIO writer tests including round-trip through system `cpio`, 3 resolver-precedence tests). Zero regressions. **Phase 8 mission statement met + over-delivered**: `elastos setup --profile minimal && elastos run ubuntu-base` brings up a fully booted Ubuntu LTS on Mac, login-prompt-ready. See [`PHASE_8_DAY_1_NOTES.md`](PHASE_8_DAY_1_NOTES.md) artifact decision, [`PHASE_8_DAY_2_NOTES.md`](PHASE_8_DAY_2_NOTES.md) install wiring, [`PHASE_8_DAY_3_NOTES.md`](PHASE_8_DAY_3_NOTES.md) doctor row, [`PHASE_8_DAY_4_NOTES.md`](PHASE_8_DAY_4_NOTES.md) substrate boot, [`PHASE_8_DAY_5_NOTES.md`](PHASE_8_DAY_5_NOTES.md) one-command CLI demo, and [`PHASE_8_DAY_6_NOTES.md`](PHASE_8_DAY_6_NOTES.md) for the writable-overlay design + both smoke captures. Day 7 closed the interactive-console gap: `elastos run ubuntu-base` now drops the operator at a real `root@ubuntu:/#` shell on macOS, not just a buried `ubuntu login:` tracing event. The Phase-2-dormant `VmConfig.interactive_stdio` knob was lit up via a new `build_interactive_kernel_console()` FFI branch (`dup` stdin/stdout → bidirectional `VZFileHandleSerialPortAttachment` → `closeOnDealloc` is safe on the dups) that flips the existing pipe-backed console to operator-direct-attach when stdin is a TTY. The standalone lane (`run_microvm_standalone`) detects TTY via `enable_host_raw_mode_pub()` and falls back to the Day-6 headless pipe-backed path when stdin isn't a terminal (CI / piped input), so the change is additive — every existing headless caller is byte-identical. `BuiltMachine.kernel_console_host_read` + `VzMachineHandle.forwarder` both became `Option<_>` so the lifecycle skips `console_forwarder` spawning in the interactive branch (no in-process pipe to forward). Smoke surfaced two real-substrate quirks the prompt anticipated: agetty's `--login-program /bin/bash` passes `-p -h <host> -f <user>` argv which bash misinterprets (`-h` as `hashall`, `root` as a script path → `bash: root: Is a directory`), AND Ubuntu's `/bin/login` on hvc0 re-prompts for a password even after `agetty --autologin -f root` because pam_securetty rejects the non-secure-tty AND root is locked in `/etc/shadow`. Fix: a 3-line `/usr/local/sbin/elastos-login` wrapper that discards agetty's argv and `exec -l /bin/bash`'s an interactive shell, plus a `serial-getty@hvc0.service.d/autologin.conf` drop-in pointing at the wrapper. Both written into the **tmpfs upperdir** by `/init` before `switch_root` → ephemeral per-boot, zero squashfs modifications, zero distribution payload bloat. End-to-end smoke via `/usr/bin/expect` (so the agent harness allocates a real PTY): `cat /etc/os-release` → `Ubuntu 22.04.5 LTS`; `uname -srm` → `Linux 5.15.0-179-generic aarch64`; `mount | grep overlay` → `overlay on / type overlay (rw,relatime,lowerdir=/lower,upperdir=/upper/upper,workdir=/upper/work,xino=off,nouserxattr)` — Day-6 wiring still intact end-to-end. Ctrl-C in host → SIGINT → clean VM stop → TermiosGuard restores terminal on drop. **404/404** elastos-server lib (+1 autologin drop-in test), **96/96** elastos-vz lib (+1 interactive-console-has-no-pipe regression-pin), zero regressions. See [`PHASE_8_DAY_7_NOTES.md`](PHASE_8_DAY_7_NOTES.md) for the FFI design, the agetty arg-conflict root-cause analysis, and the full smoke transcript. Remaining Phase-7-era infrastructure work (orthogonal to Phase 8): activate self-hosted Mac runner (hardware procurement only); sign `elastos-server` for distribution (needs Apple Developer ID cert). Day-8+ scope: persistent-overlay flag for workloads needing state across runs; guest-side Ctrl-C escape sequence (QEMU-style `Ctrl-A x`) so operators can interrupt guest commands without exiting the VM.** Closes the
> [`PLAN.md` § Phase 6](PLAN.md) deliverable (L331–344) and
> resolves the 3 entry-gate unblockers from
> [`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md):
> truthful `components.json` darwin-arm64 entries
> (Unblocker 1), self-hosted Mac CI runner activation
> (Unblocker 2), first end-to-end full-boot smoke green
> (Unblocker 3). Each day lands one commit + one
> `PHASE_6_DAY_N_NOTES.md` outcome log, matching the
> Phase-4/5 cadence.
>
> **Sequencing rationale.** Unblocker 1 is gated on a
> per-binary audit (no binary moves without knowing what
> it costs); Unblocker 2 is gated on Unblocker 1 (a runner
> with no workloads has nothing to validate); Unblocker 3
> is gated on both. Days 1–4 attack Unblocker 1 step by
> step. Day 5 lights up Unblocker 2. Day 6 closes
> Unblocker 3 + triages real-substrate bugs. Days 7–8
> expand perf coverage + phase closeout.
>
> **Anchor:** [`PLAN.md`](PLAN.md) § Phase 6 (umbrella),
> [`PHASE_5_RETROSPECTIVE.md`](PHASE_5_RETROSPECTIVE.md)
> § Carry-forward findings (backlog seed), this plan's
> ancestor [`PHASE_5_PLAN.md`](PHASE_5_PLAN.md)
> (template + tone).

## 1. Mission

> **Phase 6 ships ElastOS on macOS: a tagged release with a
> signed + notarized Mac binary, a `components.json` whose
> darwin-arm64 entries are truthful (point at MicroVM
> capsules that boot inside real Apple Vz), all three
> Phase-5 smokes green end-to-end on a self-hosted runner,
> and a performance baseline with real-Vz-boot
> measurements alongside the synthetic ones.**

## 2. Day-by-day

The 8 days break into four blocks: **Audit & restore
metadata (Days 1–4)**, **CI activation (Days 5–6)**,
**Perf expansion (Day 7)**, **Phase closeout (Day 8)**.

---

### Day 1 — `components.json` audit + per-binary decision matrix (4–6 h)

> **Outcome (2026-05-25):** ✅ **Audit complete; Day-2 unblocked.**
> Audit lives at
> [`PHASE_6_COMPONENTS_AUDIT.md`](PHASE_6_COMPONENTS_AUDIT.md).
> All four architecture decisions are closed:
> **(A)** `vmlinux` darwin-arm64 = build same 6.1.59 source for arm64;
> **(B)** `crosvm` darwin = omit entry (install-loop skip is graceful);
> **(C)** `kubo` / `cloudflared` / `llama-server` darwin-arm64 = ingest
> upstream macOS-arm64 builds;
> **(D)** `install.sh` upstream bash-3.2 fix deferred to Phase 7; Mac
> smokes use `ELASTOS_BIN_OVERRIDE` for Phase 6.
> Smoke surface map: `local-carrier-setup` + `home-frontdoor` +
> `chat-wasm-native-interop` collectively assert on **5 binaries**
> (`shell`, `localhost-provider`, `did-provider`, `webspace-provider`,
> `chat`) — these are Day-2's minimum-required population set.
> Day-1 diff is docs-only (this banner + the audit doc); no
> substrate touched.

**Problem.** `PLAN.md` L338 mandates *"restore the darwin
entries in `components.json` — this time truthfully,
because the capsules now run inside real microVMs."*
`PHASE_5_DAY_1_NOTES.md` documents that 11 native binaries
in `external/` currently have only `linux-amd64` +
`linux-arm64` platform entries; the three Phase-5 smokes
auto-skip on Mac because of this gap (Mac pre-flight
helper `cross_platform_assert_native_binary_release_metadata`).
We need a per-binary inventory + decision before adding
any entry, so Day 2+ does targeted work rather than
hunt-and-peck.

**Concrete deliverables:**
1. **`docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md` (new).**
   Per-binary table with columns:
   - **Name** — e.g. `did-provider`.
   - **Class** — `microvm-capsule`, `native-helper`,
     `kernel-artifact`, `linux-only-substrate`.
   - **Current platforms** — from `components.json` today.
   - **Darwin source strategy** — `share-linux-arm64-rootfs`,
     `cross-compile-from-source`, `upstream-macos-build`,
     `defer-to-phase-7`, `n/a`.
   - **Build cost estimate** — `trivial` (<30 min metadata
     update), `small` (1–4 h: rebuild + sign single
     artifact), `medium` (4–8 h: tooling work), `large`
     (>1 day, defer).
   - **Signing required?** — Yes if binary touches Vz or
     runs as a privileged process; otherwise no.
   - **Verification smoke** — which of the three Phase-5
     smokes proves it works end-to-end.
   - **Decision** — `Day 2`, `Day 3`, `Day 4`, or
     `Defer to Phase 7`.
2. **Decision: `vmlinux` darwin strategy.** Document in
   the audit doc which path we take from
   `PHASE_0_SCOPE.md` §C.3 risk register: build same
   6.1.59 source for arm64 (and pin sha256), OR pin
   Ubuntu LTS arm64 cloud-kernel checksum, OR share the
   existing `linux-arm64` artifact if it boots on Vz
   unmodified. The decision drives Day 4 scope.
3. **Decision: `crosvm` darwin marker.** It's linux-only
   by design (replaced by Vz on Mac). Two options: omit
   the darwin platform entry (Mac install will skip),
   OR add an explicit `"n/a-on-darwin"` sentinel platform
   so the operator-facing `setup --list` output documents
   the substitution. Pick one; document the choice.
4. **Decision: native helpers (`cloudflared`, `kubo`,
   `llama-server`) source.** These have upstream macOS
   distributions; document whether we ingest the upstream
   darwin-arm64 build (preferred — same trust source as
   Linux) or rebuild locally (slower; only if upstream
   gap exists).
5. **Smoke surface map.** For each of the 3 Phase-5
   smokes, list which binaries it exercises and which
   Day delivers that binary. Day 2's smoke validation
   uses this map to prove the right thing.
6. **No `components.json` changes today.** Pure audit
   day. The first modification lands in Day 2.

**Out of scope for Day 1:**
- Building any binary.
- Cross-compiling, signing, notarizing.
- Touching `components.json`.
- Activating the self-hosted CI runner.

**Anchors:**
- [`PLAN.md`](PLAN.md) L319–325, L338, L353–354 (risk
  register row).
- [`PHASE_5_DAY_1_NOTES.md`](PHASE_5_DAY_1_NOTES.md) §
  "Mac pre-flight" (the current behaviour the smokes
  exhibit on the metadata gap).
- [`scripts/lib/cross-platform.sh`](../../scripts/lib/cross-platform.sh)
  `cross_platform_assert_native_binary_release_metadata`.

---

### Day 2 — `components.json` schema edits, host-binary lane (6–8 h)

> **Outcome (2026-05-25):** ✅ **Class-A/D/E + capsules-projection
> edits landed; Day-3 unblocked.**
> Notes:
> [`PHASE_6_DAY_2_NOTES.md`](PHASE_6_DAY_2_NOTES.md).
> **2 of 3 smoke pre-flights flip SKIP → PASS** on Mac
> (`local-carrier-setup`, `home-frontdoor`); the third
> (`chat-wasm-native-interop`) is awaiting the Class-B
> `chat` capsule entry due Day 3, per the audit's
> per-Class schedule. New durable verifier
> [`scripts/lib/components-json-verify.sh`](../../scripts/lib/components-json-verify.sh)
> gates Day-3+ drift. The 3 upstream darwin-arm64 checksums
> (kubo / cloudflared / llama-server) are recorded
> verbatim in the notes for audit trail. Linux behaviour
> preserved: helper still returns 0 for both linux-amd64
> and linux-arm64 keys against the same binary lists; diff
> is purely additive (no linux-amd64 or linux-arm64 row
> deleted).
>
> **Scope deviation from original plan.** The original Day-2
> framing (below) picked one microVM capsule and walked it
> end-to-end. The Day-1 audit re-sequenced this: Class A
> (7 host binaries) + Class D + Class E + capsules
> projection are all metadata-only and ship together on
> Day 2; the microVM capsule (`chat`, Class B) walks
> end-to-end on Day 3 because it depends on a Decision-D.2
> sub-pick (duplicate vs `share_release` field). This
> re-sequencing trades the original Day-2's "one-capsule
> proof" for "everything that doesn't need a substrate
> change ships at once" — a strictly larger Day-2 deliverable.

**Problem.** Day 1 produces a decision matrix; Day 2
proves the *workflow* — pick one capsule, walk it
end-to-end from "Linux-only metadata" to "darwin-arm64
entry that the smoke validates", document any
substrate bugs that surface. Day 3 then batches the rest
of the microVM capsules with high confidence.

**Concrete deliverables:**
1. **First microVM capsule's darwin-arm64 entry in
   `components.json`.** Pick the smallest-graph capsule
   that the Phase-5 smokes exercise — likely
   `did-provider` because `home-frontdoor-smoke.sh`
   already proves the supervisor RPC contract for it on
   Mac (visibly-skipping until the metadata lands per
   [`PHASE_5_DAY_2_NOTES.md`](PHASE_5_DAY_2_NOTES.md)).
2. **Choose source strategy from Day 1's matrix.** Most
   likely: share the `linux-arm64` rootfs (since the
   microVM is a Linux guest running inside Vz; the rootfs
   bytes are the same). If a separate artifact is needed
   (e.g. signed entrypoint), document why.
3. **Validate via `home-frontdoor-smoke.sh` FORCE_FULL
   lane.** On the dev Mac (NOT the self-hosted runner —
   that's Day 5+ scope), run
   `ELASTOS_VZ_SMOKE_FORCE_FULL=1 bash scripts/home-frontdoor-smoke.sh`
   and verify it exits 0 with no `vz_error` tripwire hits
   in the captured logs.
4. **`PHASE_5_DAY_3_NOTES.md`'s install.sh blocker
   surfaces.** The Mac install.sh is bash-3.2 incompatible
   (Day 3 documented `GATEWAYS[@]: unbound variable` as a
   Phase-6 prerequisite). Day 2 EITHER fixes the install.sh
   bash-3.2 compatibility OR scopes the smoke to use
   `ELASTOS_BIN_OVERRIDE` to bypass install.sh. Choose
   based on Day 1's audit.
5. **Capture the dev-Mac smoke output** to
   `docs/vz-backend/artifacts/PHASE_6_DAY_2_did_provider_smoke.txt`
   (or whichever capsule) as audit-trail evidence the
   FORCE_FULL lane works against real metadata for the
   first time.
6. **Docs:** `PHASE_6_DAY_2_NOTES.md` capturing scope
   deviation (if any), real substrate bugs surfaced
   (expected: at least one), `vz_error` types observed,
   the workflow template for Day 3 to follow.

**Out of scope for Day 2:**
- Other microVM capsules (Day 3).
- Native helpers (Day 4).
- Self-hosted runner (Day 5).
- Code-signing the Mac binary (Day 4).
- Notarization (Day 4).

**Anchors:**
- [`PHASE_5_DAY_2_NOTES.md`](PHASE_5_DAY_2_NOTES.md) (the
  current home-frontdoor smoke skip behaviour).
- [`PHASE_5_DAY_4_NOTES.md`](PHASE_5_DAY_4_NOTES.md) (the
  supervisor's `EnsureCapsule` + orphan-cleanup behaviour
  the smoke exercises end-to-end).
- [`docs/vz-backend/PHASE_6_COMPONENTS_AUDIT.md`](PHASE_6_COMPONENTS_AUDIT.md)
  (Day 1's output; primary reference).

---

### Day 3 — Class-B `chat` capsule darwin-arm64 (share-linux-arm64-bundle) (4–6 h)

> **Outcome (2026-05-25):** ✅ **Class-B `chat` darwin-arm64
> landed via D.2.a share-bundle metadata; all 3 Phase-5 smoke
> pre-flights PASS on Mac; Day 4 unblocked.**
> Notes:
> [`PHASE_6_DAY_3_NOTES.md`](PHASE_6_DAY_3_NOTES.md).
> **Decision D.2 closed at D.2.a** (duplicate
> `linux-arm64.{cid,checksum,size,release_path,extract_path}`
> into `darwin-arm64`; zero substrate change). **D.2.b
> (`share_release` schema indirection) parked as a Phase-7
> schema-elegance carry-forward.** The verifier now enforces
> the D.2.a share-bundle invariant (5 fields must be
> byte-identical between linux-arm64 and darwin-arm64);
> negative-test confirmed the invariant catches drift.
> Class B promoted from forward-compat to required; only
> Class C remains forward-compat (Day 4 — `vmlinux`).
> Linux preserved; diff is additive only.
>
> **Scope deviation from original plan.** The original Day-3
> framing (below) was *"batch the remaining 7 microVM capsules
> at once"*. The Day-1 audit re-shaped this — most of the
> "microVM capsules" the original plan listed are actually
> **host Rust binaries** (Class A), which Day 2 already
> shipped. The only true Class-B microVM bundle is `chat`,
> which is what Day 3 actually ships. The other 6 entries
> the original plan listed (`ipfs-provider`, `tunnel-provider`,
> `site-provider`, etc.) are Class-A host binaries already
> covered by Day-2. Day 3 is therefore narrower and faster
> than the 6–8h budget — the deliverable is one capsule
> entry, one verifier promotion, one notes file.

**Problem.** Day 2 proves the workflow; Day 3 batches the
remaining 7 microVM capsules (`chat`, `ipfs-provider`,
`localhost-provider`, `shell`, `site-provider`,
`tunnel-provider`, `webspace-provider`) per Day 1's
decision matrix.

**Concrete deliverables:**
1. **7 new `darwin-arm64` entries in `components.json`.**
   Each follows the workflow Day 2 documented; entries
   group by source-strategy class to minimise repetitive
   per-binary boilerplate in the commit.
2. **Validate via `local-carrier-setup-smoke.sh`
   FORCE_FULL.** This is the longest smoke; runs the
   Carrier install + microVM launch path against
   multiple capsules at once. Expected: at least one new
   substrate bug surfaces (we've never run this end-to-end
   against multiple Vz capsules concurrently with real
   metadata before).
3. **For each substrate bug surfaced:** decide stop-now
   (fix in Day 3) OR carry-forward (Phase 6 Day 7+ or
   Phase 7). Stop-now criteria: the bug blocks the
   smoke's exit-0 path. Carry-forward criteria: smoke
   passes but a `vz_error` is captured in logs that
   warrants follow-up.
4. **Phase-5-Day-5 `mac-rust-tests` job re-runs green.**
   The new metadata triggers visibly-skipping integration
   tests (`vz_chat_interop_smoke.rs`,
   `vz_home_frontdoor_smoke.rs`) to actually exercise
   their full assertions. Test count is allowed to grow
   here as the visibly-skip paths convert to real
   assertions.
5. **Capture per-smoke FORCE_FULL output** as audit-trail
   artifacts under `docs/vz-backend/artifacts/`.
6. **Docs:** `PHASE_6_DAY_3_NOTES.md` documenting:
   workflow + outcomes, per-capsule decisions, any
   Day-2 → Day-3 workflow refinements, substrate bugs
   surfaced (with disposition: fixed / carried-forward),
   FORCE_FULL smoke artifact links.

**Out of scope for Day 3:**
- Native helpers (Day 4).
- vmlinux strategy (Day 4).
- Self-hosted runner activation (Day 5).
- Real-microVM perf measurements (Day 7).

**Anchors:**
- [`PHASE_5_DAY_3_NOTES.md`](PHASE_5_DAY_3_NOTES.md) (the
  chat-wasm-interop smoke's visibly-skip behaviour).
- [`PHASE_5_DAY_4_NOTES.md`](PHASE_5_DAY_4_NOTES.md)
  (`Supervisor::new` orphan-cleanup behaviour Day 3
  exercises N times across the batch).

---

### Day 4 — vmlinux build recipe + Mac signing/notarization scaffolding (4a) (4–6 h agent + ~5 h operator handoff)

> **Outcome (2026-05-25):** ✅ **Day 4a complete (agent-shipped
> scaffolding); Day 5 unblocked. Day-4b operator handoff queued.**
> Notes: [`PHASE_6_DAY_4_NOTES.md`](PHASE_6_DAY_4_NOTES.md).
>
> **What landed (4a):** `scripts/build-vmlinux-arm64.sh`
> (deterministic ARM64 kernel cross-compile recipe), `scripts/release-mac.sh`
> (Developer-ID codesign + notarytool submit + staple recipe),
> `scripts/release/elastos-server.entitlements.plist` (six Vz-aware
> entitlements). `external.vmlinux.platforms.darwin-arm64` added
> with the empty-cid/checksum stub pattern that Class-A host
> binaries already use. Class-C verifier promoted from
> forward-compat to required-keys-present; only operator-handoff
> note remains. Both recipes preflight-tested live on dev Mac with
> typed exit codes + clean diagnostics.
>
> **What's queued (4b — operator-side):** running the build
> recipe (requires `brew install aarch64-elf-gcc`, ~30 min wall-clock),
> populating components.json's vmlinux checksum+size from build
> output, running the signing recipe (requires Apple Developer
> Program enrollment + Developer ID cert + notarytool keychain
> profile). ~5h total operator wall-clock. Not on Day-5/6/7
> critical path.
>
> **Scope deviation from original plan.** The original Day-4 framing
> bundled three Sub-deliverables (vmlinux build, components.json
> edit, signing recipe) as one 6–8h day. Honest audit revealed:
> Sub-1 (build) and Sub-3 (signing) both have hard operator
> prerequisites (cross-compile toolchain installation, Apple
> Developer Program enrollment) that can't close in an agent
> session. The plan split into Day 4a (agent-shipped scaffolding)
> and Day 4b (operator-shipped execution). 4a still landed every
> piece of code the original prompt named; 4b is the documented
> operator-handoff queue.
>
> **Mirrors Phase 3 Day 7 precedent.** That phase explicitly
> scoped *"Mac-side release-engineering: Developer ID signing
> pipeline, entitlement plist, notarization"* out of agent scope.
> Phase 6 Day 4a finally addresses the scaffolding; Day 4b is the
> operator execution.

**Problem.** Phase 5 + Days 2–3 of Phase 6 prove the
microVM-capsule install path. Day 4 closes the remaining
metadata gaps (native helpers + kernel artifact) and adds
the release-pipeline plumbing (code-signing + notarization)
that `PLAN.md` L335 calls out as a Phase-6 mandatory.

**Concrete deliverables:**
1. **3 native helpers' darwin-arm64 entries.** `cloudflared`,
   `kubo`, `llama-server`. Per Day 1's decision matrix:
   most likely ingest upstream macOS builds (same trust
   source as Linux). Document any binary that requires
   re-signing or notarization at our layer (i.e.
   downloaded via our gateway; Apple's quarantine bit may
   need explicit clearing).
2. **`vmlinux` darwin-arm64 entry** per Day 1's decision.
   Either: share the `linux-arm64` artifact (if it boots
   on Vz unmodified — most likely outcome), publish a
   Mac-targeted rebuild (if kernel config delta is
   required), or pin an Ubuntu LTS arm64 cloud-kernel.
3. **`crosvm` darwin marker** per Day 1's decision.
4. **`just release-mac` recipe** (or scripts/release-mac.sh).
   Signs the `elastos-server` binary with Developer ID
   Application certificate, applies the hardened runtime
   + `com.apple.security.virtualization` entitlement,
   submits for notarization via `notarytool`. Recipe is
   gated on `APPLE_DEVELOPER_ID` env var being set;
   defaults to a clear `"credentials unavailable; cannot
   produce production-signed binary"` skip message so
   public CI doesn't fail loudly without credentials.
5. **Validate via `chat-wasm-native-interop-smoke.sh`
   FORCE_FULL** — exercises the full
   curl-install + native-helper path end-to-end.
6. **Update `Info.plist` / build settings to declare
   `LSMinimumSystemVersion = 12.0`** per `PLAN.md` L336.
7. **`components.json` final state matches Day 1's audit
   matrix.** Verify by re-running Day 1's audit script.
8. **Docs:** `PHASE_6_DAY_4_NOTES.md` documenting:
   release recipe + signing flow, native-helper trust
   sources, kernel artifact decision, any remaining
   metadata caveats.

**Out of scope for Day 4:**
- Self-hosted runner activation (Day 5).
- Real-microVM perf (Day 7).
- Notarization-failure recovery automation (Phase 7 if
  it surfaces as a real operational pain point).

**Anchors:**
- [`PLAN.md`](PLAN.md) L335 (code-sign + notarize
  mandate), L336 (min-version mandate).
- [`PHASE_5_DAY_3_NOTES.md`](PHASE_5_DAY_3_NOTES.md) §
  Carry-forward (install.sh bash-3.2 incompatibility —
  may need a small touch here if Day 2 didn't fix it).
- [`docs/MAC.md`](../MAC.md) § "binary signing
  requirements" (the entitlement check substrate from
  Phase 3 Day 7).

---

### Day 5 — Self-hosted Mac runner activation (5a) (3–4 h agent + ~20 min operator handoff)

> **Outcome (2026-05-25):** ✅ **Day 5a complete (agent-shipped
> scaffolding); Day 6 unblocked modulo ~20 min operator wall-clock.
> Day-5b operator handoff queued.**
> Notes: [`PHASE_6_DAY_5_NOTES.md`](PHASE_6_DAY_5_NOTES.md).
>
> **What landed (5a):** `scripts/ci/setup-mac-runner.sh` (one-command
> preflight: HW/OS floors, toolchain install, Vz framework check,
> components.json verifier delegate, Day-4b artefact probe, operator
> handoff block). Spec [`SELF_HOSTED_RUNNER_SPEC.md`](SELF_HOSTED_RUNNER_SPEC.md)
> promoted from "wired but dormant" to "recipe available; pre-flight
> no longer skips" — new § 4.5 documents the recipe + typed exit
> codes (0..4). Runbook [`CI_RUNBOOK.md`](CI_RUNBOOK.md) § 3a.2 +
> § 5 status table refreshed. Recipe live-tested on dev Mac with
> clean `exit=0`.
>
> **What's queued (5b — operator-side):** procure Apple-Silicon Mac
> matching spec § 2 floors; run `bash scripts/ci/setup-mac-runner.sh`;
> register runner with `self-hosted,macOS,ARM64,vz-capable` labels;
> `gh variable set MAC_VZ_FULL_BOOT_ENABLED --body true`; trigger
> `_self-hosted-probe.yml` for the first probe-attempt green. ~20 min
> active operator time excluding HW procurement.
>
> **Scope deviation from original plan.** The original Day-5 prompt's
> 8 gates required physical HW + GitHub registration token + repo-admin
> credentials — none agent-attainable. Day 5a/5b split mirrors Day 4a/4b
> precedent: agent ships reproducible recipe, operator runs it. The
> original 8 gates are still covered (4 in 5a as agent-side; 4 in 5b
> as operator-side). Spec/runbook/notes/banner updates land in 5a; the
> first runner activation lands in 5b.
>
> **Day-4b + Day-5b ordering note.** Either order works. Day 5b first
> means `mac-vz-full-boot` exits with typed "vmlinux not found" until
> Day 4b lands; Day 4b first means the runner has its kernel ready
> the moment Day-5b activates it. The recipe handles either order via
> its informational-only Day-4b artefact probe.

**Problem.** Day 4 closes Unblocker 1 structurally; Day 5 lights up
Unblocker 2 per
[`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md)
§ Unblocker 2. Without a self-hosted runner activated,
the `mac-vz-full-boot` job (Phase 5 Day 6 deliverable)
is dormant; CI is dry-run-only.

**Concrete deliverables:**
1. **Self-hosted runner provisioned.** Apple Silicon Mac
   per [`SELF_HOSTED_RUNNER_SPEC.md`](SELF_HOSTED_RUNNER_SPEC.md)
   hardware + OS requirements (≥16 GiB RAM, ≥100 GiB
   disk, macOS 13+).
2. **Runner agent installed** with label set
   `[self-hosted, macOS, ARM64, vz-capable]`.
3. **Repository variable set:**
   `gh variable set MAC_VZ_FULL_BOOT_ENABLED --body true`.
4. **Heartbeat probe** (`_self-hosted-probe.yml` from
   Phase 5 Day 6) shows green on the
   `probe-attempt` job within 24 h of activation.
5. **First `mac-vz-full-boot` job triggered** via
   `workflow_dispatch:`. May fail on a real Vz substrate
   bug; that's expected on first run + becomes Day-6
   triage scope.
6. **`SELF_HOSTED_RUNNER_SPEC.md` post-flight section
   added** documenting the actual provisioning experience
   (any gaps in the spec become Day-5 doc fixes).
7. **`CI_RUNBOOK.md` § 3a updated** with the activated-
   lane's actual operator workflow (any pre-vs-post
   activation deltas).
8. **Docs:** `PHASE_6_DAY_5_NOTES.md` documenting:
   provisioning timeline, hardware specs (so future
   capacity planning has a reference), first-run
   outcomes, carry-forward bugs from the first job
   triggered.

**Out of scope for Day 5:**
- Fixing every bug the first `mac-vz-full-boot` job
  surfaces (Day 6).
- Multi-runner fleet (Phase 7).
- Auto-recovery / agent monitoring (Phase 7).

**Anchors:**
- [`SELF_HOSTED_RUNNER_SPEC.md`](SELF_HOSTED_RUNNER_SPEC.md)
  (primary reference for provisioning).
- [`CI_RUNBOOK.md`](CI_RUNBOOK.md) § 3a (operator
  workflow).
- [`PHASE_5_DAY_6_NOTES.md`](PHASE_5_DAY_6_NOTES.md)
  (gating + precedence design that Day 5 activates).

---

### Day 6 — First end-to-end FORCE_FULL smoke green (6a local-lane scaffolding) (3–4 h agent + ~45 min operator handoff)

> **Outcome (2026-05-25):** ✅ **Day 6a complete (agent-shipped
> scaffolding); Day 7 unblocked modulo Day-6b operator handoff.**
> Notes: [`PHASE_6_DAY_6_NOTES.md`](PHASE_6_DAY_6_NOTES.md).
>
> **What landed (6a):** `scripts/release/vmlinux-arm64.config` (the
> missing kconfig fragment that gated Day-4b — Vz-required CONFIG_*
> overrides + capsule-isolation primitives + rootfs prereqs), modified
> `scripts/build-vmlinux-arm64.sh` to use the canonical kernel-build
> pattern (`make defconfig` → `merge_config.sh -m` →
> `make olddefconfig`), and `scripts/ci/local-day6-smoke.sh`
> (one-command orchestrator: preflight delegate, cargo build, vmlinux
> probe, per-smoke FORCE_FULL run with env-var matrix, structured
> triage summary). Live-tested stages 1–3 on dev Mac with clean
> typed exit-3 on missing vmlinux.
>
> **What's queued (6b — operator-side):** `brew install
> aarch64-elf-gcc make elfutils openssl@3 bc jq`, then `bash
> scripts/build-vmlinux-arm64.sh` (~30–40 min), stage Image at
> `~/.local/share/elastos/bin/vmlinux`, then `bash
> scripts/ci/local-day6-smoke.sh` for the first 3/3 green. ~45–55 min
> single-sitting modulo per-iter cycles if real-Vz substrate bugs
> surface.
>
> **Scope deviation from original plan — lane reframing.** Original
> Day-6 framing assumed `mac-vz.yml::mac-vz-full-boot` would have
> produced a terminal first-run by now (gated on Day-5b's self-hosted
> runner registration). Phase-6 audit revealed the runner is not the
> cheapest substrate for *first-green* — the dev Mac in hand has the
> same Vz API surface. Day-6 reframed as a **local-lane substrate
> validation** with the self-hosted CI lane deferred to Phase 7 as
> gating-CI work. Same Vz API contract; same kernel Image; same
> elastos-server binary; just no separate runner machine needed for
> the headline-gate "first FORCE_FULL smoke green" outcome.
>
> **Day-4b absorbed into Day-6b.** Day-4a's operator queue
> (`vmlinux-arm64.config` derivation + `build-vmlinux-arm64.sh` run)
> is now a single `bash scripts/build-vmlinux-arm64.sh` invocation —
> Day-6a shipped the previously-missing fragment + recipe modification.
> Day-4b's signing/notarization sub-task (Gate 4b-6) remains queued
> as Phase-7 work — the local lane doesn't require signed binaries.

**Problem.** Day 5 activates the runner; Day 5's first
`mac-vz-full-boot` job invocation likely surfaces 1+ real
substrate bugs the dev-Mac smokes on Days 2–4 missed
(different host config, different concurrency profile,
different residual state). Day 6 closes Unblocker 3 by
triaging + fixing those bugs OR demonstrating they're
out-of-scope environmental issues.

**Concrete deliverables:**
1. **All 3 Phase-5 smokes pass on the self-hosted
   runner** with `ELASTOS_VZ_SMOKE_FORCE_FULL=1`. This is
   the headline gate for Unblocker 3.
2. **Per-substrate-bug triage:** for each issue surfaced
   by Day 5's first `mac-vz-full-boot` run, document
   root cause, fix (or carry-forward decision), and the
   regression test that locks it in.
3. **New Mac-specific integration tests** for any
   substrate bugs that warrant in-process regression
   coverage (target ≤3 new tests; aggressive scope
   discipline — most fixes should be 1-line fixes with
   the smoke-level gate as the proof, not a new Rust
   test).
4. **Per-smoke wall-clock measurement.** The
   self-hosted runner now reports first real end-to-end
   timings; capture them in `PERFORMANCE_BASELINE.md`'s
   comparison table (Day 7 expands this with the perf
   harness's real-microVM-boot path).
5. **CI green on `main`** after the bug-fix commits land
   — `mac-vz.yml` full-boot job stays green for at
   least one push cycle.
6. **Docs:** `PHASE_6_DAY_6_NOTES.md` documenting: each
   bug + fix + cost, the new green-state baseline, any
   real-Vz substrate insights worth capturing for Phase 7.

**Out of scope for Day 6:**
- Perf measurement (Day 7).
- Notarization-credential automation (Day 4 + Phase 7
  if it surfaces).
- Multi-capsule concurrent stress (Phase 7).

**Anchors:**
- [`PHASE_5_DAY_5_NOTES.md`](PHASE_5_DAY_5_NOTES.md) (CI
  workflow design).
- [`PHASE_5_DAY_6_NOTES.md`](PHASE_5_DAY_6_NOTES.md)
  (full-boot job + precedence).
- [`PHASE_4_DAY_7_NOTES.md`](PHASE_4_DAY_7_NOTES.md) (the
  `VzError` taxonomy Day 6 will lean on for substrate-bug
  classification).

---

### Day 7 — Real-microVM perf measurement (6–8 h)

**Problem.** Phase 5 Day 7 shipped a synthetic perf
harness; `notes.real_vz_boot_measured: false` was the
Phase-5 honesty marker. Phase 6 Day 6 unlocks the
substrate that makes real Vz boots measurable. Day 7
expands `vz_perf_harness.rs` to include the
`Supervisor::ensure_capsule` → real `LaunchMicroVm` path
under both Vz and crosvm.

**Concrete deliverables:**
1. **`vz_perf_harness.rs` gains a `perf_real_microvm_boot`
   metric** that exercises the full real-Vz launch path
   (against the now-truthful `did-provider` from Day 2).
   Metric definition:
   - Cold launch: time from `LaunchCapsule` RPC enter to
     first heartbeat from the guest, fresh data-dir.
   - Warm launch: same against a populated data-dir
     (capsule artifact already cached).
2. **Linux companion runs the same metric against
   crosvm.** Apples-to-apples Mac vs Linux number
   captured in `PERFORMANCE_BASELINE.md`.
3. **Schema bumped to v3** (additive). New top-level
   `real_microvm_boot_measured: true` field +
   per-metric category (`synthetic` vs `real`). v2
   consumers continue to parse v3 files; v3-aware
   consumers can filter on category.
4. **`notes.real_vz_boot_measured` flips to `true`** in
   the Mac baseline JSON. The Phase-5 honesty marker
   resolves.
5. **`PERFORMANCE_BASELINE.md` § Comparison table** —
   Linux column populated; real-boot rows added; the
   `_TBD_` cells from Phase 5 Day 7 close.
6. **Sanity-check existing 6 synthetic metrics** for
   regressions. The DRY-hoisted `tests/common/mod.rs`
   makes the fixture surface the same as Phase 5; any
   delta is a real signal.
7. **Docs:** `PHASE_6_DAY_7_NOTES.md` documenting:
   methodology delta from Phase 5 Day 7 (the additive
   parts), measured Mac vs Linux numbers + per-metric
   observations (cold/warm delta, NSFileHandle limits if
   relevant, GCD queue contention if relevant), schema
   v3 contract.

**Out of scope for Day 7:**
- Bridge code / `TxExecutable` perf metrics (Phase 7
  carry-forward).
- CI regression-detector (Phase 7 carry-forward).
- N-concurrent-VM stress beyond what Day 6 already
  validated (Phase 7).

**Anchors:**
- [`PHASE_5_DAY_7_NOTES.md`](PHASE_5_DAY_7_NOTES.md)
  (perf-harness substrate Day 7 expands).
- [`PERFORMANCE_BASELINE.md`](PERFORMANCE_BASELINE.md) §
  "What we cannot measure yet" (the section Day 7
  empties).
- [`PHASE_5_DAY_8_NOTES.md`](PHASE_5_DAY_8_NOTES.md) §
  Perf-report schema v1→v2 (the v2 → v3 evolution
  template).

---

### Day 8 — Phase 6 closeout + tagged release (4–6 h)

**Problem.** Days 1–7 ship the work; Day 8 ships the
*release* and the closeout artefacts so Phase 7 starts
with the same discipline Phase 5 started with.

**Concrete deliverables:**
1. **`docs/vz-backend/PHASE_6_RETROSPECTIVE.md` (new).**
   Same structure as
   [`PHASE_5_RETROSPECTIVE.md`](PHASE_5_RETROSPECTIVE.md):
   what we set out to do, what shipped (table per day),
   final state, scope deviations, what went well, what
   didn't, carry-forward findings.
2. **`docs/vz-backend/PHASE_7_ENTRY_CHECKLIST.md` (new).**
   Same structure as
   [`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md):
   Phase-6 closeout gates + Phase-7 unblockers (likely:
   darwin-amd64 Intel Mac support, bridged-network
   entitlement reactivation, CI regression detector,
   bridge/TxExecutable perf metrics) + Phase-7 backlog.
3. **`state.md` § Support boundary** updated to add
   macOS (`aarch64-apple-darwin`) as a truthful full-
   runtime target — per `PLAN.md` L339.
4. **`docs/PC2_CONVERGENCE.md` Slice C/D** closed out
   per `PLAN.md` L340.
5. **`PRINCIPLES.md` #10 audit line** added per
   `PLAN.md` L342: *"MicroVM substrate is now
   `crosvm + KVM` on Linux, `Apple Vz` on macOS — one
   canonical path per platform, no soft alternates."*
6. **Release notes** drafted per `PLAN.md` L341.
7. **Git tag.** Format: `v<existing-numbering>-mac-vz`
   (or whatever the existing release scheme dictates —
   confirm with operator before tagging).
8. **`PLAN.md` § Phase 6 status banner** → "✅ Phase 6
   complete".
9. **`docs/MAC.md` capability matrix** final pass: every
   row referencing Phase 5 gets the ✅ marker; Phase 6
   row gets the same.
10. **Docs:** `PHASE_6_DAY_8_NOTES.md` capturing the
    closeout (just like
    [`PHASE_5_DAY_8_NOTES.md`](PHASE_5_DAY_8_NOTES.md)).

**Out of scope for Day 8:**
- Pushing to the production release channel (operator
  decision; the tag exists locally + on the remote, but
  the gateway-side publish is a separate gated action).
- Phase 7 day-1 work.
- Marketing / external-comms outside the release notes
  draft.

**Anchors:**
- [`PLAN.md`](PLAN.md) L338–344 (Phase 6 deliverable
  list — Day 8 closes the last items).
- [`PHASE_5_DAY_8_NOTES.md`](PHASE_5_DAY_8_NOTES.md)
  (template for closeout cadence).
- [`PRINCIPLES.md`](../../PRINCIPLES.md) #10 (the
  audit line to add).

---

## 3. Risk register

| Risk | Severity | Mitigation |
|---|---|---|
| One microVM capsule's `linux-arm64` rootfs fails to boot on Vz | High | Day 2 catches this against the first capsule; Day 3 has a kernel-config-delta investigation branch documented in advance. |
| Apple Developer ID + notarization credentials not available | Medium | Day 4 `just release-mac` recipe skips with a clear message if creds absent; dev-Mac smokes don't require notarized binaries; Day 8 release tag is creds-gated. |
| Self-hosted runner provisioning takes longer than 1 day | Medium | Day 5 buys a half-day buffer in the 4–6 h estimate; if provisioning slips, Day 6 starts on a dev Mac and the self-hosted lane validates after-the-fact. |
| First `mac-vz-full-boot` job surfaces a >1 day substrate bug | High | Stop, file a follow-up day at the boundary, do NOT scope-creep Day 6. Phase 4–5 set this precedent. |
| `vmlinux` arm64 needs a Mac-specific kernel rebuild | Medium | Day 1's audit catches this; Day 4 budget absorbs a small rebuild; a large rebuild defers to Phase 7. |
| Real-microVM perf numbers reveal a 5×+ Vz vs crosvm gap | Medium | Document honestly per principle #12 (`PLAN.md`); accept; Phase 7 considers tuning. |
| Code-signing breaks the entitlement check substrate from Phase 3 Day 7 | High | Day 4 explicitly re-runs the entitlement check unit tests after signing; mismatch blocks the recipe. |
| `state.md` Support boundary update reveals docs drift across the rest of the repo | Low | Day 8 fixes the boundary update only; broader drift is a Phase 7 doc-pass deliverable. |

## 4. Out of scope for Phase 6 (deferred to Phase 7+)

- **`darwin-amd64` (Intel Mac).** `PLAN.md` L337 +
  `PHASE_5_PLAN.md` § Out of scope. Phase 6 ships Apple
  Silicon only.
- **Bridged-network (`com.apple.vm.networking`) entitlement
  reactivation.** Phase 3 Day 7 gated the substrate; Phase
  6 doesn't unblock it (`PLAN.md` L261, L361).
- **Persistent `VzErrorReport` history across supervisor
  restarts.** Phase 4 Day 8 deferral reaffirmed in
  `PHASE_5_PLAN.md`; Phase 6 doesn't unblock it.
- **CI regression-detector** comparing perf baseline
  deltas. Carry-forward from `PHASE_5_RETROSPECTIVE.md`.
- **`just verify` Mac parity recipe.** Phase 5 Day 8
  deferral; re-evaluate after Phase 6 ships if a
  developer-facing one-stop-shop is still worth it.
- **Bridge code + `TxExecutable` perf metrics.**
  Phase 7 carry-forward from Phase 5 Day 8.
- **MCP / agent tooling for the perf harness.** Phase 5
  Day 8 carry-forward; re-evaluate after Phase 6.
- **Multi-runner fleet + auto-recovery.** Phase 5 Day 6
  deferral; Phase 7 if operational pain materialises.

## 5. Success criteria

By end of Phase 6:

1. **`just verify` green on `aarch64-apple-darwin`.**
   Per `PLAN.md` L400. Maps onto Day 6 (the lane that
   proves smokes pass end-to-end) + Day 8 (`state.md`
   update closing the support-boundary gap).
2. **Every smoke from `state.md` L29–50 has a passing
   Mac variant.** Per `PLAN.md` L401. Maps onto Day 6.
3. **Same capsule artifacts run on Mac and Linux with
   identical isolation guarantees.** Per `PLAN.md` L402.
   Maps onto Days 2–3 (microVM capsules) + Day 4
   (native helpers).
4. **`components.json` darwin-arm64 entries are truthful.**
   Per `PLAN.md` L403. Maps onto Days 1–4 (the metadata
   restoration).
5. **`docs/MAC.md`, `state.md`, and `setup --list` agree
   on the Mac story.** Per `PLAN.md` L404. Maps onto
   Day 8 (the audit pass).
6. **Real-Vz-boot perf measurements alongside synthetic
   ones.** Maps onto Day 7. Closes the
   `notes.real_vz_boot_measured: false` Phase-5 marker.
7. **Tagged release with signed + notarized Mac binary.**
   Maps onto Day 4 (signing pipeline) + Day 8 (tag).
8. **Phase 6 retrospective + Phase 7 entry checklist
   shipped.** Maps onto Day 8.

## 6. Phase 6 entry signal

Phase 6 Day 1 starts when all three of the following are
true (re-stated from
[`PHASE_6_ENTRY_CHECKLIST.md`](PHASE_6_ENTRY_CHECKLIST.md)):

- [ ] Phase 5 closeout gates all pass (9 items in the
      checklist).
- [ ] Operator has reviewed
      [`PHASE_5_RETROSPECTIVE.md`](PHASE_5_RETROSPECTIVE.md)
      § Carry-forward findings.
- [ ] Operator has reviewed this plan.

The 3 Phase-6 unblockers in the entry checklist do NOT
need to be resolved before Day 1 — Days 1–4 attack
Unblocker 1, Day 5 attacks Unblocker 2, Day 6 attacks
Unblocker 3. That's the plan's whole structure.

## 7. Estimated total effort

| Day | Range | Cumulative low | Cumulative high |
|-----|-------|----------------|-----------------|
| 1   | 4–6 h | 4 h            | 6 h             |
| 2   | 6–8 h | 10 h           | 14 h            |
| 3   | 6–8 h | 16 h           | 22 h            |
| 4   | 6–8 h | 22 h           | 30 h            |
| 5   | 4–6 h | 26 h           | 36 h            |
| 6   | 6–8 h | 32 h           | 44 h            |
| 7   | 6–8 h | 38 h           | 52 h            |
| 8   | 4–6 h | 42 h           | 58 h            |
| **Total** | **42–58 h** | | |

Comparable to Phase 5's 40–55 h budget; in line with
`PLAN.md`'s "1 week" estimate (a focused full week at
~50 h is the canonical fit).
