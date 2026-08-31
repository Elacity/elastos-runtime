# Agent And Operator Process

This file is the durable working contract for people and agents changing this
repo. Product principles live in [PRINCIPLES.md](PRINCIPLES.md), current truth
lives in [state.md](state.md), and open work lives in [TASKS.md](TASKS.md).

## User-Facing Communication

- Use ASD-STE100 Simplified Technical English for user-facing communication.
- Describe the intended behavior first, with clear actors and ownership.
- Express security and architecture boundaries as positive ownership statements.
  Example: "Runtime keeps provider routes private" instead of a list of places
  where routes must not appear.
- State a current gap once, then explain the desired behavior. Do not repeat the
  same limitation in several forms.
- Use connected, natural paragraphs. Avoid staccato status sentences, chains of
  negation, and long prohibition lists.
- Before sending a public comment, community update, handoff, or summary, apply
  the humanizer skill and rewrite repeated uses of "not", "no", "never", and
  "must not" as direct statements of behavior or ownership.
- Keep technical precision without turning every sentence into a disclaimer.

## Branch Roles

- `main` is the stable source line.
- `upstream/<version>-dev` branches are development integration lines. Resolve
  the active line from `state.md` and fetched refs before choosing a base.
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

New feature or bugfix branches start from a user-selected `upstream/XX-dev`
integration branch. The current checkout and `main` are not implicit bases:

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

## Local Hygiene And Retention

Local refs, worktrees, build outputs, proof directories, installed artifacts,
and rollback copies are operational state with an explicit lifecycle. Creating
them creates a cleanup obligation; "temporary" is not a lifecycle.

- Keep a local, untracked operator ledger for every non-canonical branch,
  worktree, detached HEAD, large proof directory, installed runtime, and
  rollback set. Record its exact path or ref, commit and tree where applicable,
  owner or purpose, dirty state, creation or observation date, protection
  source, and one terminal decision: keep, review/merge, archive, or remove.
  Never commit private paths or operator details from this ledger.
- Keep local branches only for active development or valuable work that is not
  protected by a fetched remote ref or intentional tag. Do not create
  `backup/*`, `archive/*`, timestamped safety, or sync refs for clean or already
  published commits. A temporary preservation ref must name the dirty or
  unanchored state it protects, have a ledger cleanup condition, and be removed
  as soon as that condition is met.
- Do not leave an unanchored detached HEAD. Before ending the task that creates
  one, either attach it to an intentional branch/tag, prove that another ref
  contains it, or record it as an explicit preservation blocker in the local
  ledger.
- A temporary worktree must be removed before handoff unless its ledger entry
  names the active task and cleanup condition. Keep at most one worktree for a
  branch. A clean checkout is reproducible state, not a backup.
- Before deleting a duplicate ref, prove exact commit identity. Same-tree but
  different-history refs are not automatic deletion candidates: preserve or
  explicitly waive the unique history first. Before deleting a worktree,
  verify its status, untracked files, open files/processes, and protecting ref.
- Build outputs, Cargo targets, dependency caches, VM hibernation state, and
  proof scratch directories are rebuildable artifacts, not rollback copies.
  Do not retain them merely because they were expensive to create.
- Never run an installed or long-lived Runtime from `/tmp`, `/private/tmp`, a
  Cargo `target` directory, or another disposable checkout. Install it to a
  stable data path with a receipt binding source commit, source tree, binary
  SHA-256, components manifest, capsule tree, and installation time.
- Retain no rollback by default. A named risky deployment, migration, or
  incident may temporarily keep at most one verified rollback for the affected
  release/platform; it needs a receipt, size, reason, explicit expiry or cleanup
  condition, and must be removed when that gate closes. Do not recursively copy
  a live data root without an explicit size estimate and exclusion list for
  existing backups, VM images, caches, identity state, and user data.
- Maintain at least 10% free space on development and staging volumes. If free
  space falls below that threshold, stop creating worktrees, builds, VM images,
  and backups until the ledger is reconciled and safe reclaim has completed.
- Before handoff, rerun the branch/worktree inventory, check every touched
  worktree for dirt, report local/remote divergence, and update the local
  ledger. Do not describe a cleanup as complete while an unexplained ref,
  detached HEAD, worktree, active temporary binary, or unbounded rollback set
  remains.

## Publishing Terms And Gates

Use precise verbs. If the user says "publish" without a target, restate the
target before acting.

- prepare: make a local, reviewable commit or commit set; do not push or deploy.
- publish for review: push the named local branch to the named remote only after
  reporting commits, divergence, and verification.
- deploy live: update the approved public Home target from a named commit and
  verify the served artifact hashes before moving `live`.
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
- Keep standing instructions, READMEs, contracts and roadmaps independent of
  the current release. Branch heads, PR numbers, commit IDs, CI results and
  installation snapshots belong in `state.md` or a dated evidence record.
  Contract versions, dependency pins and artifact provenance stay with the
  contract or artifact they identify.

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

## Journey Register Gate

Product-behavior changes and releases are measured against the journey audit
register [docs/audits/ElastOS-Home-Journey-Audit.xlsx](docs/audits/ElastOS-Home-Journey-Audit.xlsx),
the standing reference for stability and correctness for any PR or release.
Before declaring product work done or a release ready, load the
[e2e-audit skill](.claude/skills/e2e-audit/SKILL.md) and apply its
invariant: read the touched areas' journeys and findings, keep Pass journeys
stable, update rows a change intentionally alters in the same PR, add rows for
new surface, and close findings only with their recorded proof. Live verdicts
come from installed evidence on the exact revision, never from source reading.

## Public Live Deployment

The public live host must preserve its data root, signing key, passkey state, and
provider config while replacing only intentional release artifacts.

Any public-live mutation requires explicit user approval before the mutation,
even when a dry-run plan reports ready artifacts.

Set the deployment inputs from the reviewed target configuration:

- gateway root: `$ELASTOS_LIVE_HOME`
- data root: `$ELASTOS_LIVE_XDG_DATA_HOME/elastos`
- local Home URL: `$ELASTOS_LIVE_LOCAL_URL`
- public Home URL: `$ELASTOS_LIVE_PUBLIC_URL`

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
curl -fsS -o /dev/null -w '%{http_code}\n' "$ELASTOS_LIVE_LOCAL_URL"
curl -fsS -o /dev/null -w '%{http_code}\n' "$ELASTOS_LIVE_PUBLIC_URL"
curl -fsS "$ELASTOS_LIVE_PUBLIC_URL" | sha256sum
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

The Browser product contract is WebRTC remote display through the
Runtime Browser Engine Adapter with Runtime-only networking and explicit Browser
Engine/Exit service selection. `runtime_frame`, `diagnostic_frame`, screenshot,
and image-polling display paths are removed from the product path and must not be
reintroduced as compatibility fallbacks.

Mac VZ and Linux/crosvm Jetson are host adapters behind the same Browser/Net/
Exit/Wallet contracts. A non-KVM server acts as a gateway and remote-engine
consumer. Local Browser VM proof requires a suitable host. Host-specific launchers are
implementation details behind Runtime contracts, not separate Browser products.

Native Browser helpers and hosted/Selkies proof tooling may exist, but native,
hosted, macOS, Linux, Jetson, arbitrary media, wallet-dapp, or microVM Browser
support is not accepted from source presence alone. Product Browser readiness
requires target evidence for audio/video/input, frame continuity, heartbeat and
reconnect behavior, explicit close/orphan cleanup, and wallet dapp flows, plus a
hash-bound manual UX report where required.

A macOS `.dmg` support claim requires a stable
macOS source-home path, provider binaries, launch wrapper, passkey/origin policy,
update story, and human Home/app/chat proof. Do not conflate `.dmg` packaging
with Browser engine isolation or product media proof.

Cosmopolitan Libc may be researched for small C/C++ helper binaries, but it is
not a drop-in answer for Rust workspace packaging, Chromium, WebView, GPU/audio,
microVM isolation, or `.dmg` distribution.
