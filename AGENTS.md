# Agent And Operator Process

This file is the durable working contract for people and agents changing this
repo. Product principles live in [PRINCIPLES.md](PRINCIPLES.md). GitHub issues
are the only place for work status, acceptance criteria and proof.

## Work Tracking And Acceptance

- GitHub issues own the work queue. In each lane, take the lowest-numbered open
  task in the current milestone whose dependencies are satisfied and
  which has no active blocker. Keep at most one task issue In progress per
  lane. Parent issues group the lane;
  they do not authorize a second active task. User messages approve, redirect
  or reorder work; they do not create a parallel task list.
- Close a task only when its Done means is proven and its PR has the required
  reviews and is merged. Until then, keep verified work open in QA / Review.
  Take the next unblocked task in that lane; starting it does not complete its
  predecessor. Keep blocked work open with its blocker in the issue.
- At the first CI failure caused by your candidate, cancel its remaining jobs
  and repair the cause before another push. Continue eligible work while CI
  runs. If CI is the only remaining work, end the turn and check again in
  15 minutes. Record an external blocker in the owning issue.
- Run at most one heavy local build on this Mac at a time. Workers agree which
  issue owns that build before starting it. CI builds do not count toward this
  local limit. Continue reviews, source work or other light checks in parallel.
- Comment on GitHub only for a decision, a result with its link, a blocker or a review verdict.
  Keep comments to at most 3 lines: no narration, hashes, receipts or agent-to-agent coordination.
  Put status in the issue checklist.
- Keep one work record. Do not create briefings, plans, gists or notes files.
  Move useful open work from dated audits to its issue.
- Keep seed security details, credentials and private operator data out of
  public issues, PRs and logs. Give the public issue only a safe summary and
  the approval or verification still needed. Retain sensitive raw evidence
  privately under its existing access controls.
- Every Done means includes the complete user journey. A visible feature must
  work well from start to end, with clear controls and recovery. Keep an
  unfinished or confusing feature hidden until that journey passes. Hiding a
  feature does not complete its task. Source checks alone do not prove the
  installed product works.
- Durable product instructions, licences, test fixtures and artifact provenance
  stay with their code or package. They do not carry a second work queue.

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

- `main` is the release line.
- `develop` is the integration line. Task PRs merge into it when their checks
  and reviews pass (see Review And Commit Discipline).
- Feature and fix branches remain unpublished working lines until they are
  explicitly pushed for review.
- Do not assume a `review/*` or `live` ref exists. Identify the exact public
  review or deployed commit from fetched refs and target-host evidence before
  making either claim.
- Verify the exact branch, commit, tree id, dirty status and command before
  treating a local branch or target-host checkout as evidence. Keep these
  identities in the lifecycle inventory or the installed-artifact receipt;
  routine check-ins link the related PR, CI run and review.
- Always report remote divergence. A local branch being green is not the same as
  the remote branch being up to date.

## Creating Work Branches

Every task branch uses [develop](https://github.com/Elacity/elastos-runtime/tree/develop)
as its base and PR target unless the owning issue names another base. This is
standing approval; do not ask for each task. Fetch the current base before
branching. If the base moves, check compatibility before merging; history
rewrite still needs approval. A branch that depends on another in-flight branch
says so in its PR and merges after its parent. Name branches `feat/<slug>` or
`fix/<slug>`. Pushes and upstream tracking follow the Push Approval Levels below.

Canonical workflow: [.claude/skills/branching-strategy/SKILL.md](.claude/skills/branching-strategy/SKILL.md).

## Releases

Releases are built by `.github/workflows/release-package.yml` and published with `scripts/release-publish.sh` as described in [Publishing a release](docs/VERSIONING.md#publishing-a-release).
Changes to `main` and tags stay with Anders.

## Branch Lifecycle

Retire a branch or close a PR only with its owner's agreement. A green check,
merge or duplicate finding does not replace that agreement.

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

- Keep a local, untracked machine-readable inventory for every non-canonical branch,
  worktree, detached HEAD, large proof directory, installed runtime, and
  rollback set. Record its exact path or ref, commit and tree where applicable,
  owner or purpose, dirty state, creation or observation date, protection
  source, and one terminal decision: keep, review/merge, archive, or remove.
  This lifecycle inventory is not a status or proof log. Keep private paths and
  operator details private. References to the local ledger below mean this inventory.
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
- Stop each test Home and its owned child processes when its test ends. Keep
  its data until its retention decision is made. Remove completed-test login
  agents; an agent kept for an active test must name the owning GitHub issue.
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

### Push Approval Levels

- **No approval needed:** fast-forward pushes to the branch of the active task
  or its existing draft PR; opening draft PRs; rerunning checks; reading logs.
  Report what was pushed, including the remote, branch, commits and check results.
- **Standing task-merge permission:** merge your task PR into `develop`, or
  into the user-approved base named in its issue, when all current checks pass,
  the independent reviews below pass, findings are resolved, dependencies are
  integrated, and any explicit human acceptance gate passes. Check current-base
  compatibility and report links to the merged PR, CI run and reviews. Use a
  normal merge; history rewrite needs approval.
- **Standing test-key permission:** create and use disposable keys and signatures
  for a named, isolated test fixture. Keep its trust store separate from real
  Homes and production keys. Record its owner and cleanup condition; keep private
  keys out of source, logs and reports. This permission covers fixture keys only.
- **Ask first:** force-push or history rewrite; merges into other bases; changes
  to `main`; tags and releases; production signing, including release and catalogue
  keys; changes on the seed or existing installed Homes; deleting branches;
  other secrets and credentials.

The operator runs production signing with Anders's approval. Named isolated
fixtures may generate disposable keys and signatures in CI; their trust stays
separate from real Homes and production keys. Fork PRs and publication jobs use
GitHub-hosted runners.

These levels replace blanket push-approval requirements. Verification and the
agreed publication scope still apply. Keep unrelated work and private operational
records outside the published candidate.

Before any remote push, show:

```bash
git log --oneline <upstream>..HEAD
git diff --stat <upstream>...HEAD
git rev-list --left-right --count <upstream>...HEAD
```

Before each push, fetch and merge current `origin/develop`, then check the
clean candidate with `just ci-local-prepush`. The committed pre-push hook
checks the exact pushed HEAD and refuses a stale base. It runs formatting,
workspace/all-targets checks, Clippy for touched crates, and their unit targets.
Use the [local pre-push procedure](scripts/README.md#local-pre-push-gate) to
activate the hook and share the local heavy-build lease across worktrees.
Reproduce an unclear failed Mac install, update, or Home startup step locally
under that lease before pushing. Review a large diff with Opus before long Mac
CI. Record the command and result in the owning issue; source checks and an
operator reminder alone do not prove that installed step.

Before deploying public Home, show the exact commit being deployed, confirm that
`live` either already points to that commit or will be moved only after
successful verification, and preserve a rollback path for the installed binary,
capsules, provider config, and `components.json`.

## Review And Commit Discipline

- Keep review requests off draft PRs. When a PR leaves draft, request the
  required reviewers on GitHub yourself and name them in the report. Keep an
  existing request instead of sending a duplicate.
- Each task change needs one independent development review marked PASS.
  Security code, including gateway, auth, provider launchers, confinement,
  signing, publish/update, install.sh and workflows, also needs a posted
  `Independent adversarial review ... VERDICT: PASS` from a different model
  family than its author. Codex authors run `elastos-pr-review <PR>` themselves
  and verify that the posted verdict covers the current candidate.
- Agents merge their own task PRs after the current checks and reviews pass.
  Keep explicit human acceptance gates recorded in the owning issue. Request
  `irzhywau` for security follow-up: he reviews merged security task PRs, and a
  finding goes into an existing issue or a revert when it reopens a hole.
  When he authors a security task PR, Anders conducts the follow-up review.
  Release PRs still need his approval before merge and tag; when he is
  the author, Anders reviews. Also request the feature owner when required.
- Use one development review round per coherent change. Resolve findings in
  that round and fix their cause before requesting the current verdict. Scope
  decisions belong to Anders. Ask before adding new diagnostic tooling.
- Keep commits authority-bound and reviewable: one coherent concern per commit,
  with its own verification commands.
- Preserve commit history by default. Ask before amending, squashing, rebasing
  or otherwise rewriting commits, including an unpublished tail.
- If a correction belongs with the previous unpublished commit, propose folding
  it into that concern for review. Obtain approval before rewriting the history.
- Do not delete or rewrite dirty worktrees unless the user explicitly approves
  it. If duplicate trees exist, prove byte identity and clean status before
  recommending deletion.
- Keep release history in `elastos/CHANGELOG.md` and product instructions in
  the relevant documentation.
- Keep product documentation release-neutral. Use version numbers only for
  exact release state, compatibility or migration, release notes or changelogs,
  manifests, and exact Git refs or evidence. Keep standing instructions,
  READMEs, contracts, and roadmaps independent of the current release.
  Branch heads, PR numbers, commit IDs, CI results, and installation snapshots
  belong in the owning GitHub issue. Contract versions,
  dependency pins, and artifact provenance stay with the contract or artifact
  they identify.

## Verification Gate

Use the smallest checks that cover the touched surface, but do not skip the
basic gate before handing work back:

```bash
git diff --check
node scripts/check-product-data.mjs
node --test scripts/check-product-data.test.mjs
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
```

Run Rust workspace commands from `elastos/`, not the repo root. Add narrow tests
for touched crates or scripts, for example:

```bash
(cd elastos && cargo test -p elastos-server people_discovery -- --nocapture)
cargo test --manifest-path capsules/chain-provider/Cargo.toml -- --nocapture
```

For Browser-facing changes, include the relevant Browser behaviour/smoke gates and
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

Prove installed behavior on the CI install jobs (GitHub-hosted Linux x86-64,
Linux ARM64 and macOS machines) when they cover the journey, and link that CI
run as the proof. Otherwise use the installed-proof steps above. The Jetson is
checked on the release candidate. A PR that changes what users see links the CI
screenshots at desktop and phone width.

## Journey Register Gate

Use [docs/audits/ElastOS-Home-Journey-Audit.xlsx](docs/audits/ElastOS-Home-Journey-Audit.xlsx)
and the [e2e-audit skill](.claude/skills/e2e-audit/SKILL.md) as historical test
input. Transfer relevant journeys and open findings to the owning GitHub issue.
Keep accepted journeys working, add acceptance checks for changed behavior,
and record results in that issue. Installed acceptance requires evidence from
the exact candidate and human checks where specified.

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

- retain a rollback only when approved under Local Hygiene And Retention;
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
worktree paths, or operator usernames. Keep those details in private operational
configuration and pass them through explicit environment variables or CLI flags.

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

