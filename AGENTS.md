# Agent And Operator Process

This file is the durable working contract for people and agents changing this
repo. Product principles live in [PRINCIPLES.md](PRINCIPLES.md), current truth
lives in [state.md](state.md), and open work lives in [TASKS.md](TASKS.md).

## Branch Roles

- `main` is the stable release line; it currently represents 0.5.0.
- `upstream/0.6-dev` is the current 0.6 development integration line.
- Feature and fix branches remain unpublished working lines until they are
  explicitly pushed for review.
- Do not assume a `review/*` or `live` ref exists. Identify the exact public
  review or deployed commit from fetched refs and target-host evidence before
  making either claim.
- Treat any other local branch or target-host checkout as evidence only after
  reporting its exact branch, commit, tree id, dirty status, and verification
  command.
- Always report remote divergence. A local branch being green is not the same as
  `elacity/<branch>` being up to date.

## Creating Work Branches

New feature or bugfix branches start from an `upstream/XX-dev` integration
branch (e.g. `upstream/0.6-dev`) — never silently from `main` or from whatever
branch happens to be checked out. The user chooses the base; do not pick it
yourself:

- Run `git fetch origin --prune`, then list `upstream/*-dev` lines and active
  `feat/*`/`fix/*` branches before proposing bases.
- Ask the user which base to start from, offering each `upstream/XX-dev`
  (newest recommended) plus "an existing working branch" for work that depends
  on another in-flight branch. Ask even when only one dev line exists.
- If the chosen base is a working branch rather than `upstream/XX-dev`, warn
  immediately: the new branch must merge back only after (or together with)
  its parent, merging it into `upstream/XX-dev` may conflict with the parent's
  changes, and it needs a rebase if the parent moves.
- Name branches `feat/<slug>` or `fix/<slug>`; do not push or set upstream
  tracking until asked.

Canonical workflow: [.claude/skills/branching-strategy/SKILL.md](.claude/skills/branching-strategy/SKILL.md).

## Branch Lifecycle

Before creating, deleting, merging, or publishing branches, produce a short
branch inventory:

```bash
git status --short --branch
git branch --list --format='%(refname:short) %(objectname:short) %(upstream:short) %(subject)'
git worktree list
```

Every active local branch must have a role. If the branch is not `main`, `live`,
or the current development line, classify it before doing more work:

- unique work to merge;
- byte-identical duplicate of another branch;
- dirty worktree to preserve;
- backup branch kept only until the user confirms cleanup.

Delete no branch or worktree until its tree identity and dirty state are known.
For same-tree checks, compare tree objects, not just commit subjects:

```bash
git rev-parse <branch>^{tree}
git rev-parse <target>^{tree}
git diff --stat <target>...<branch>
```

Avoid creating timestamped backup branches during normal work. If a backup is
unavoidable, name the reason, keep a cleanup task with it, and remove it after
the protected work is merged or proven duplicate.

## Publishing Terms And Gates

Use precise verbs. If the user says "publish" without a target, restate the
target before acting.

- prepare: make a local, reviewable commit or commit set; do not push or deploy.
- publish for review: push the named local branch to the named remote only after
  reporting commits, divergence, and verification.
- deploy live: update `https://elastos.elacitylabs.com/apps/home/` from a named
  commit and verify the served artifact hashes before moving `live`.
- release: merge to `main`, update release notes/version/tag, and push only
  after the release gate passes.

Default safety rule: code is not pushed, deployed, tagged, or merged to `main`
unless the user explicitly asks for that action after seeing the relevant local
state and verification result. "Looks good" after reviewing one commit is not
permission to publish unrelated remaining commits.

Before any remote push, show:

```bash
git log --oneline <upstream>..HEAD
git diff --stat <upstream>...HEAD
git rev-list --left-right --count <upstream>...HEAD
```

Before deploying public Home, show the exact commit being deployed, confirm that
`live` either already points to that commit or will be moved only after
successful verification, and preserve a rollback path for the installed binary,
capsules, provider config, and `components.json`.

## Review And Commit Discipline

- Keep commits authority-bound and reviewable: one coherent concern per commit,
  with its own verification commands.
- Do not hide corrective commits. If a reviewed commit must be repaired before
  publish, fold the repair into the coherent slice before asking for review.
- Preserve reviewed history by default. When a branch has a reviewed prefix and
  an unpublished tail, reorganize only the unpublished tail unless the user
  explicitly asks to redo the whole branch.
- If a commit is too small, badly titled, or only fixes the immediately previous
  unpublished commit, merge it into that unpublished slice before review instead
  of publishing a corrective follow-up.
- Do not delete or rewrite dirty worktrees unless the user explicitly approves
  it. If duplicate trees exist, prove byte identity and clean status before
  recommending deletion.
- Avoid volatile proof logs in durable docs. Store open work in `TASKS.md`,
  verified current truth in `state.md`, and release history in
  `elastos/CHANGELOG.md`.

## Verification Gate

Use the smallest checks that cover the touched surface, but do not skip the
basic gate before handing work back:

```bash
git diff --check
node scripts/home-entropy-check.mjs
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
```

Run Rust workspace commands from `elastos/`, not the repo root. Add narrow tests
for touched crates or scripts, for example:

```bash
(cd elastos && cargo test -p elastos-server people_discovery -- --nocapture)
cargo test --manifest-path capsules/chain-provider/Cargo.toml -- --nocapture
```

For Browser-facing changes, include the relevant Browser entropy/smoke gates and
do not claim product readiness unless `scripts/browser-objective-audit.mjs`
passes with accepted product media plus matching manual UX evidence.

For installed provider changes, also prove the installed binary and manifest:

```bash
scripts/installed-provider-verify.sh <provider>
```

For installed or served capsule/runtime changes, source tests are not enough.
Before declaring a live localhost fix verified, prove artifact parity for the
path the user is actually running. Report the edited source path, built artifact
path, installed artifact path, SHA-256 of the built and installed artifact,
restart or stale-process cleanup performed, and the live localhost proof command
and result.

## Public Live Deployment

The public live host must preserve its data root, signing key, passkey state, and
provider config while replacing only intentional release artifacts.

Any public-live mutation requires explicit user approval before the mutation,
even when a dry-run plan reports ready artifacts.

Current public-live convention:

- gateway root: `$ELASTOS_LIVE_HOME`
- data root: `$ELASTOS_LIVE_XDG_DATA_HOME/elastos`
- public URL: `https://elastos.elacitylabs.com/apps/home/`

For source-home rebuilds, keep `HOME` and `XDG_DATA_HOME` pointed at the live
root, but pin Rust tooling to the real toolchain. Otherwise `rustup` can look in
the live home and miss installed targets such as `wasm32-wasip1`.

```bash
HOME="$ELASTOS_LIVE_HOME" \
XDG_DATA_HOME="$ELASTOS_LIVE_XDG_DATA_HOME" \
CARGO_HOME="$ELASTOS_OPERATOR_CARGO_HOME" \
RUSTUP_HOME="$ELASTOS_OPERATOR_RUSTUP_HOME" \
PATH="$ELASTOS_OPERATOR_CARGO_HOME/bin:$PATH" \
ELASTOS_QUIET_RUNTIME_NOTICES=1 \
scripts/setup-source-home.sh
```

`setup-source-home.sh` builds native provider binaries, builds first-party WASM
capsules, installs app capsule trees with their root WASM entrypoints, stamps
`components.json`, and prepares source-home runtime helpers. Before restart:

- back up the live binary, `components.json`, provider config, and capsule tree;
- install the rebuilt `elastos/target/release/elastos`;
- keep Browser supervisor scripts on a stable live-data path, not a temporary
  checkout path.

After restart, verify:

```bash
curl -fsS -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8090/apps/home/
curl -fsS -o /dev/null -w '%{http_code}\n' https://elastos.elacitylabs.com/apps/home/
curl -fsS https://elastos.elacitylabs.com/apps/home/ | sha256sum
sha256sum \
  "$ELASTOS_LIVE_XDG_DATA_HOME/elastos/capsules/home/browser/index.html" \
  capsules/home/browser/index.html
```

Review the new gateway log for provider verification warnings, signer DID
mismatches, invalid Home launch tokens, and app-launch `400`/`500` errors before
declaring public live ready.

## Staging Machines

Use target roles consistently:

- public server: public live proof and non-KVM gateway/remote-engine consumer;
- Mac: staging, macOS VZ Browser proof, and cross-platform proof;
- Jetson: Linux/crosvm native Browser target and intended main device proof.

Mac staging requires durable SSH before serious testing. `tmate` is acceptable
only as a break-glass bootstrap channel. During that bootstrap, create or reuse a
dedicated staging account, install an agent-owned public key in
the target account's authorized-keys file, disable password assumptions, record a local SSH host
alias, and verify non-interactive commands work. If no durable SSH is available,
say the Mac is blocked instead of implying it was verified.

Do not commit staging aliases, private key names, reverse-tunnel ports, local
worktree paths, or operator usernames. Keep those details in local operator
notes and pass them through explicit environment variables or CLI flags.

Target proof must cite the exact source tree, target-local commit or artifact
receipt, and verification command used for the run. Do not treat a missing
active Browser page as a passing Browser product proof.

## Browser Claim Discipline

The current 0.5.0 Browser product contract is WebRTC remote display through the
Runtime Browser Engine Adapter with Runtime-only networking and explicit Browser
Engine/Exit service selection. `runtime_frame`, `diagnostic_frame`, screenshot,
and image-polling display paths are removed from the product path and must not be
reintroduced as compatibility fallbacks.

Mac VZ and Linux/crosvm Jetson are host adapters behind the same Browser/Net/
Exit/Wallet contracts. This public server is a non-KVM gateway and remote-engine
consumer, not a local product Browser VM provider. Host-specific launchers are
implementation details behind Runtime contracts, not separate Browser products.

Native Browser helpers and hosted/Selkies proof tooling may exist, but native,
hosted, macOS, Linux, Jetson, arbitrary media, wallet-dapp, or microVM Browser
support is not accepted from source presence alone. Product Browser readiness
requires target evidence for audio/video/input, frame continuity, heartbeat and
reconnect behavior, explicit close/orphan cleanup, and wallet dapp flows, plus a
hash-bound manual UX report where required.

A macOS `.dmg` is a packaging goal, not current support. It requires a stable
macOS source-home path, provider binaries, launch wrapper, passkey/origin policy,
update story, and human Home/app/chat proof. Do not conflate `.dmg` packaging
with Browser engine isolation or product media proof.

Cosmopolitan Libc may be researched for small C/C++ helper binaries, but it is
not a drop-in answer for Rust workspace packaging, Chromium, WebView, GPU/audio,
microVM isolation, or `.dmg` distribution.

## Cursor Cloud specific instructions

This section is durable guidance for Cloud Agents. The startup update script
already installs `just` (`cargo install just`) and the `wasm32-unknown-unknown`
Rust target; do not re-document dependency installation here.

- Toolchain: Rust is pinned by [rust-toolchain.toml](rust-toolchain.toml)
  (1.89.0 with `rustfmt`/`clippy`) and `rustup` auto-syncs it on first cargo
  use. `wasm32-unknown-unknown` is the capsule Component target; the release
  scripts also add it on demand via `ensure_rust_target_installed`.
- Core gate has no JS/Python package deps: the `.mjs` checks in `just verify`
  (`home-entropy-check.mjs`, `browser-entropy-check.mjs`,
  `browser-window-close-handshake.test.mjs`, etc.) and the Python smokes use
  only stdlib/`node --test`. `playwright` appears only as a scanned string in
  entropy checks and as an import in Browser/GUI headless smokes that are NOT
  part of the core gate, so no `npm install` is needed for build/lint/test.
- Build/lint/test run from the repo root (the recipes `cd elastos`
  themselves): `just build`, `just lint`, `just test`. A cold `just build` is
  slow (runtime crate ~6 min). Each provider under `capsules/` is its own cargo
  project with a separate `target/` dir, so `just home-frontdoor-smoke` and
  `build.sh --all` recompile shared deps per capsule.
- Running the app from source: stage the built `localhost-provider` into
  `$XDG_DATA_HOME/elastos/bin/` and write a `components.json` beside the data
  dir whose `external.localhost-provider.platforms.<platform>.checksum` matches
  that binary's `sha256:`, then `elastos serve --addr 127.0.0.1:<port>`. The
  runtime verifies every staged component against that checksum (no dev bypass).
  It then comes up healthy (`/api/health` → `{"status":"ok",...}`), Carrier P2P
  online; authenticate via `POST /api/auth/attach` using `attach_secret` from
  `$XDG_DATA_HOME/elastos/runtime-coords.json`. Self-contained commands
  (`elastos identity show`, `elastos identity nickname set`, `elastos init`)
  work directly against an isolated `HOME`/`XDG_DATA_HOME`.
- Known release-gated gap (not an environment bug): `just home-frontdoor-smoke`
  and the `public-install-*` smokes run `scripts/install.sh`, which downloads
  the PUBLISHED release binary from `https://elastos.elacitylabs.com` (currently
  v0.1.2). That public binary lacks newer subcommands this dev tree's
  `install.sh` calls (e.g. `principal-root-upgrade`), so the installed-path
  proofs fail until a 0.6-compatible release is published. See
  [state.md](state.md) ("Public Install Truth"). Use the source `elastos serve`
  path above to run the runtime locally instead.
- No KVM: crosvm/microVM paths (Browser VM, full-screen chat microVM) warn and
  fail closed here, so product Browser proof and microVM chat are not runnable
  in this environment.
