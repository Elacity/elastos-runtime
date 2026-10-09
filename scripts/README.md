# Scripts

The `scripts/` root contains commands that developers or operators invoke
directly. Subdirectories contain implementation helpers. A root script is not
automatically a stable end-user command.

## Main entry points

- `build.sh` builds Runtime and capsules.
- `install.sh` runs the signed installer.
- `setup-source-home.sh` builds and provisions a source Home. Full mode
  installs one stable Runtime under the platform data root and writes the
  versioned source-home installation receipt.
- `home-demo-local.sh` and `chat-demo-local.sh` start disposable local demos.
- `share-demo.sh` runs the focused sharing demo.
- `setup-crosvm.sh` installs VM prerequisites.
- `publish-release.sh` prepares unsigned native release inputs. Use
  `elastos publish-release --version <version> --dry-run` for read-only planning.
  Runtime imports frozen signed output through `--signed-publication`.
- `release-signer.py` is the separately installed custodian tool. The operator
  pins its interpreter, OpenSSL and source, and owns all release-key access.
  The existing release-input CI suite runs its refused-case tests.
- `python3 scripts/publish-platform-artifacts-test.py` checks its local platform
  manifest exports, native/guest target selection and the staged artifact gate
  without signing or uploading. CI and `just verify` run these checks.
- `components-release-integrity-check.py --artifact-root <directory> --platform
  <platform> --manifest <manifest>` verifies hashes and sizes for locally
  advertised release files before publication.
- `vendor-walletconnect-adapter.sh` refreshes the pinned WalletConnect asset.

Use the `justfile` for repository gates:

```bash
just verify
just verify-release
```

`just verify` is the source gate. It runs documentation and product alignment,
versioning, WIT and template checks, Home and Browser entropy checks, command
audits, formatting, Clippy, Runtime workspace tests, own-workspace capsule
tests, and the separate Browser local-exit checks. `just verify-release` adds
browser-based UI source checks, local Carrier setup and Home front-door proofs.
Publishing trust and signer verification are separate release gates.

## Local pre-push gate

Commit the candidate, then run `just ci-local-prepush`. Resolve the source and
base before Git resolves the push object. The hook fetches develop and keeps the
checked HEAD fixed. It accepts an older develop base only when the candidate
merges cleanly with current develop and develop changed none of the candidate's
files since the merge base; otherwise merge develop first. The required PR checks
prove the merged result; this gate checks only the pushed HEAD. A moving base, a
dirty source, hidden index flags, or a pushed object from another worktree stops
the push. The gate skips `cargo clean -p` for repository packages only when
`CARGO_BUILD_BUILD_DIR` lies inside this worktree, and keeps it for the shared
default `target-build` beside the common Git directory.

The gate runs the product-data checks, `cargo fmt --all -- --check` for Runtime,
chain-provider and checked workspaces, then `cargo check --workspace
--all-targets` for Runtime, touched standalone workspaces and related consumers.
A Runtime Rust source or input change triggers metadata discovery of tracked
standalone workspaces. Declared path dependencies identify direct and transitive
consumers, including dependencies through other Runtime members. These consumers
receive workspace/all-targets checks. Clippy and unit tests cover directly
touched crates and Runtime consumers of changed product data or scripts;
dependent consumers retain their own behavior acceptance gates. Shared Runtime
WIT, configuration, manifest and lockfile inputs select every Runtime package
for Clippy and unit tests. Literal repository file and specific directory
references identify Runtime consumers of embedded data, component and model
catalogues, capsule manifests and Browser assets. Runtime script filename
literals also resolve chained joins beneath the repository scripts directory.
References inside consumed scripts and capsule tools cover their local helper
dependencies. Each uncertain product
input widens selection to all Runtime packages; generic unreferenced CI,
Python and Node tools retain their own checks. Capsule template manifests are input data;
the gate checks their Runtime consumers and keeps them outside product crate
discovery.
The gate uses Cargo metadata to select touched crates for Clippy with warnings
denied and their enabled lib/bin unit targets. It verifies conventional Rust module declarations
and available test names before selecting exact test names for a module. Module names and
unit-test names can differ, so uncertain mappings widen to a full unit target
or touched crate. Test listing and a positive passed-test count protect against
zero-test success. Broad selection keeps that refusal for a target with zero
enabled tests; source test attributes alone do not prove host coverage.
Selected server process tests first build their three
candidate provider inputs and set their test paths under the same lease.
Workspace manifests, lockfiles and Rust configuration widen the Rust scope.
Unrelated standalone workspaces keep their own checks. Documentation changes
retain the Runtime check and skip standalone dependency discovery. Capsule
suites and Linux/Browser journeys keep their own gates. A
removed package needs an explicit acceptance plan. Run the small decision
fixtures with `just test-ci-local-prepush`; the source CI gate also runs them.
Each fixture clears Git's repository-local environment variables before it
creates its owned repository. A disposable sentinel regression verifies that
running a fixture from a hook preserves the caller's HEAD, tree, index and status.
The gate retains hook Git variables for its own repository checks and clears
Git's full repository-local variable list from product child environments.
Ambient `ELASTOS_TEST_*_BIN` paths carry no candidate receipt, so the gate clears
them before checks. Its provider preparation supplies candidate test paths;
other explicit binary inputs need a source receipt and the owning test contract.

Each worktree and each local heavy operator shares the persistent lock file
`local-ai-heavy-build.lock` in `git rev-parse --path-format=absolute
--git-common-dir`. Acquire its exclusive `fcntl.flock` lease before a heavy
command and keep the file after releasing the lease. A busy lease stops the
gate. It runs Cargo commands in sequence and settles its child command on
interruption. The gate owns the lease descriptor and closes it after settling
the command group; child commands receive no lease descriptor. Operators keep
heavy tools in that group rather than detaching them. Continue light source work and reviews while another operator
owns the lease. The default shared intermediate directory is `target-build`
beside the common Git directory. An explicit `CARGO_BUILD_BUILD_DIR` selects
another shared directory; each workspace keeps its own final target directory.
The gate sets both Cargo target-directory environment variables to that
workspace's `target` directory. Before compile checks, it cleans the complete
resolved repository path-package scope with `cargo clean -p`, retaining the
shared directory. The caller-selected directory is the repository's rebuildable
intermediate cache. Each clean removes all compiled artifacts with the selected
repository package or crate names in that profile, including matches from other
resolved graphs. Artifacts with distinct package names and distinct normalized
crate names, plus registry source/download caches, stay preserved. Prepared
process providers receive the same package-scoped clean in the release profile
before their builds. An external dependency package-name collision, or a
normalized crate-name collision with its library or proc-macro target, stops
the gate before cleaning that scope. This check uses the current resolved
graph. Declared foreign tests, examples, benches and ordinary binaries are
outside the dependency compile graph and its target-name collision check.

Full metadata resolution keeps committed workspace locks fixed with `--locked`.
Ignored generated locks follow normal online Cargo resolution and can update
when their manifests change. An untracked, unignored workspace lock path stops
the gate before resolution creates source dirt. The gate logs each resolved
lock's SHA-256 with its candidate receipt. Cold caches can fetch dependencies
during metadata resolution; the following package clean uses `--locked --offline`.

If the cause of your last failed Mac install, update, or Home startup step is
unclear, reproduce that exact step locally before the next push. Hold the same
lease, use the task's approved isolated fixture, and record the candidate,
command and result in its issue. Repair the failure, then repeat that step.
The hook prints this operator requirement; it does not claim an installed
journey passed. Review a large diff with Opus before starting long Mac CI. At
the first CI failure caused by your candidate, cancel its remaining jobs. If CI is the only
remaining work, end the turn and check again in 15 minutes.

Activate the committed hook in each owned worktree after reviewing it. The
following recipe requires worktree configuration and preserves existing hooks.
When it stops for an existing hook configuration, keep that configuration and
agree how its owner will chain both hooks before changing it. The recipe changes
only the current worktree's hook path:

```bash
python3 - <<'PY'
from pathlib import Path
import subprocess

def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()

root = Path(git("rev-parse", "--show-toplevel"))
desired = root / ".githooks"
enabled = subprocess.run(["git", "config", "--bool", "--get", "extensions.worktreeConfig"],
                         text=True, stdout=subprocess.PIPE)
if enabled.stdout.strip() != "true":
    raise SystemExit("Enable worktree configuration through the repository owner first.")
configured = subprocess.run(["git", "config", "--get", "core.hooksPath"],
                            text=True, stdout=subprocess.PIPE)
if configured.returncode == 0:
    existing = Path(configured.stdout.strip()).expanduser()
    existing = existing if existing.is_absolute() else root / existing
    if existing.resolve() != desired.resolve():
        raise SystemExit("Preserve the existing hook configuration and agree hook chaining first.")
else:
    default = Path(git("rev-parse", "--path-format=absolute", "--git-path", "hooks"))
    if default.exists() and any(not p.name.endswith(".sample") for p in default.iterdir()):
        raise SystemExit("Preserve the existing hooks and agree hook chaining first.")
if not (desired / "pre-push").is_file():
    raise SystemExit("The reviewed committed pre-push hook is required.")
subprocess.run(["git", "config", "--worktree", "core.hooksPath", str(desired)], check=True)
PY
```

## Native platform inputs

Run the preparation worker from one reviewed, clean checkout on each native
builder. It supports Linux x86_64, Linux ARM64 and macOS Apple silicon. Choose
an absent output directory outside the checkout:

```sh
scripts/prepare-release-platform.sh --version VERSION --output /path/to/new-platform-input
python3 scripts/release-platform-input.py verify /path/to/new-platform-input
```

The worker builds native Runtime/provider files with locked dependencies, copies
tracked app sources and rebuilt entrypoints, and writes unsigned local inputs.
Home CLI is a platform-specific capsule archive that includes its native renderer
at `home-cli/bin/home-cli`; both setup and Runtime use that installed path.
The receipt binds the source commit/tree, lockfiles, tool versions, component
template and each output's size/hash. It records helpers absent from the source
platform matrix. Generic provider VM archives remain a separate build path.
Linux preparation needs tracked lockfiles for the standalone Browser helper
projects; a missing lockfile stops preparation before a native build.

The Browser guest is one ARM64 Chromium/Selkies image shared by Mac and Linux
ARM64. Its release artifact is `browser-vm-image-arm64.tar.gz`; both platform
rows bind the same bytes. Linux x86-64 is a remote Engine consumer.
The guest builder defaults to a 4 GiB root disk (`--rootfs-size 4096M`). Linux
preparation keeps one spare root image. Capacity proof includes the compressed
archive, extraction, profile, launch copies and update staging plus the Runtime
reserve; image qualification runs on both hosts.

The guest builder hashes its recipe, pinned Selkies inputs, guest helper sources
and build options. Running it again with the same output directory reuses an
intact image with the same `inputs_sha256`; changed inputs require a new build.
Image admission checks that identity against the candidate. Source options and
payload hashes stay in its build receipt. Legacy receipts keep their historical
proof and need explicit qualification before entering a new signed candidate.

For Mac and Linux ARM64, add `--browser-vm-image-set /path/to/verified-image-set`.
Browser preparation packages the verified four-member rootfs/kernel/initrd set
and matching host helpers in this same input. Reuse the qualified image when its
guest inputs match. An existing package can instead be supplied with
`--browser-vm-image PATH --browser-vm-image-sha256 HEX`. Image qualification runs
on the target outside CI. The package checker verifies the receipt and every
member before native compilation starts.

Pinned build recipes supply Node, TURN, Python, debugfs and Linux ARM64 crosvm.
The source builds retain their inputs, licences and native library audit. The
Python package retains the distribution's complete notices and omits the terminal
database for its noninteractive helpers. Native Linux builds need a musl C/C++
toolchain with Linux UAPI headers and the Rust musl target; crosvm also needs
libclang for bindgen. Its pinned libcap source supplies the static capability
library. The worker leaves seccomp compilation to pinned Minijail and enforces
its policies; an ambient policy compiler stops preparation.
Linux UAPI inputs come from the worker's system headers, or the explicit
`ELASTOS_BROWSER_LINUX_UAPI_INCLUDE` root. The capsule retains these headers and
locked Cargo sources with normalized archive ownership.
TURN preparation requires Perl, make, Autoconf, Automake and GNU Libtool before
compilation starts.
The build worker uses `zstd` to read the pinned Python notice archive. These are
build tools; normal setup obtains the prepared payloads from the signed release.

From that same clean candidate checkout, check all three transferred inputs:

```sh
python3 scripts/release-platform-input.py validate-inputs \
  --input x86_64-linux=/path/to/linux-x86_64-input \
  --input aarch64-linux=/path/to/linux-arm64-input \
  --input aarch64-darwin=/path/to/mac-arm64-input
```

This checks source and local file agreement, native OS/CPU headers, archive
contracts and required Home delivery metadata. Source-template external downloads
retain their pinned URLs and checksums. Media prerequisites, Browser substrate
provisioning and actual fresh-device installation keep their target acceptance.
Publisher import, signing and promotion follow this preparation boundary and
remain separate release work. These commands perform local file operations;
the builds can fetch Cargo dependencies. Keep their output through candidate
review, then remove it after adoption or abandonment.

For a staged update that keeps its support inventory fixed, prepare the next
Runtime with the first version's verified native input:

```sh
scripts/prepare-release-platform.sh --version NEXT_VERSION \
  --reuse-support /path/to/first-platform-input --output /path/to/next-platform-input
python3 scripts/release-platform-input.py verify /path/to/next-platform-input
```

This path builds only Runtime. It copies the exact component manifest, provider,
capsule and catalogue files from the first input. Both inputs use the same
platform and component template. The next receipt binds its Runtime source and
version, and `support_origin` binds the first receipt stored as
`support-input.json`. Use an original native input as the support source; a
receipt that already reuses support is refused. Keep both receipts with the
staging handoff so the operator can verify the support's original qualification.
Use the same installer source blob and public bootstrap stamps for both signed
sets to keep their installer bytes fixed. Signing and publication still follow
the separate operator procedure below.

## Public-install proof

The three public-install wrappers cover separate installed paths:

- `public-install-identity-smoke.sh`: identity and profile
- `public-install-home-frontdoor-smoke.sh`: setup and Home
- `public-install-operator-smoke.sh`: installed operator and update commands

Set `ELASTOS_PUBLISHER_GATEWAY=<url>` to test a published candidate.

During candidate review, the identity and Home wrappers accept
`ELASTOS_BIN_OVERRIDE=<path-to-branch-elastos>` only when the gateway serves a
compatible manifest with the current `home` setup profile and checksummed
artifacts. They pin the installer-selected components manifest so source
checkout metadata cannot leak into installed-path proof. The operator wrapper
always uses installed binaries and does not accept the override.

Before a candidate gateway exists, use
`scripts/local-carrier-setup-smoke.sh`. Set
`ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only for a stricter publisher
relay-health check.

The full release order and manual target pass are in the
[source integration and release checklist](../docs/RUNTIME_REPO_USER_STORY_CHECKLIST.md).

## Focused proof

Use the runbook that owns the surface:

- [Browser capsule](../docs/BROWSER_CAPSULE.md)
- [Browser VM target](../docs/BROWSER_VM_TARGET.md)
- [Inspector testing](../docs/INSPECTOR_TESTING.md)
- [People and conversations](../docs/PEOPLE_CONVERSATIONS.md)
- [Protected content](../docs/PROTECTED_CONTENT.md)
- [Capsule authoring](../docs/CAPSULE_AUTHORING.md)

Common branch gates include:

- `auth-wallet-focus-smoke.sh` for passkey, Recovery Kit, Wallet, chain, and
  principal-bound launch checks
- `wallet-product-safety-smoke.sh` for product Wallet release safety
- `wallet-connector-transaction-smoke.mjs` for fake-DOM, fake-provider
  connector handoff source proof, not hosted Browser acceptance
- `protected-content-provider-contract-smoke.sh` as the fail-closed retirement
  guard for the provisional rights, key, decrypt, and DRM providers; it does not
  verify the canonical v1 custody path
- `protected-content-installed-e2e-proof.sh` drives the installed two-Runtime
  protected-content journey phase by phase (provision, preflight,
  chain-config-real, wallet-setup, mint, availability, buy, open,
  drill-custody, drill-replica, negative, restart, cleanup, finalize, all)
  against a real installed client Runtime and the `deploy/custody-host/`
  three-node harness; its own `--help` documents the full runbook order,
  including the Home-token login sequence the HTTP journey phases need.
  `protected-content-installed-e2e-proof-smoke.sh` is its no-docker,
  no-live-gateway smoke: driver syntax, usage/phase coverage, and argument
  handling only, not the journey itself
- `people-conversations-local-smoke.sh` for profile, discovery, contacts, and
  Chat handoff
- `capsule-inspector-act-check.sh` for Inspector scope and Inbox approval
- `installed-provider-verify.sh` for an installed provider manifest and binary
- `source-home-capsule-inventory-smoke.py` for source-home capsule finalization
- `protected-content-installed-static-audit.py` for a bounded read-only audit
  of source identity, installed artifacts, private provider declarations, and
  operator prerequisites; `ready_for_active_proof` is not product readiness

`public-copy-entropy-check.mjs` checks selected public manifests, static HTML,
accessibility labels, and Home CLI command copy for Home, People, Spaces,
Services, and System. `node scripts/check-product-data.mjs` validates capsule
authority and interface metadata, setup profiles, release asset metadata,
model catalog bindings, and install icons. Behaviour tests prove Runtime
authority and UI journeys.

## Browser capacity proof

The [Browser capsule](../docs/BROWSER_CAPSULE.md) and
[Browser VM target](../docs/BROWSER_VM_TARGET.md) own the contract. These two
proof modes are easy to confuse:

```bash
# Read the current capacity receipt without opening an engine session
HOME_VIRTUAL_AUTH_BROWSER_OPEN=0 HOME_VIRTUAL_AUTH_BROWSER_SUMMARY=1 \
  node scripts/home-passkey-virtual-auth-smoke.mjs

# Open a page and test active capacity
scripts/browser-session-capacity-smoke.sh
```

The active smoke opens a page, holds heartbeats for 30 seconds, confirms that an
extra open fails with `browser_capacity_unavailable`, closes the page, and
checks that capacity returns to its starting value. Override concurrency only
when the provider truthfully supports more active pages.

Hosted Browser service files under `scripts/system/` are proof and operator
packaging. They are not the product Browser path. Private SSH aliases, users,
ports, and data roots belong in local operator notes.

## Live and recovery helpers

`mac-source-home-restart.sh` and `linux-source-home-restart.sh` restart only the
stable source-home Runtime after they validate the installation receipt,
exact prior process, bounded rollback, and served artifact parity. Mac default
mode uses the existing installation; `--init` also requires current clean
source and artifact parity. Linux writes
`receipts/linux-source-home-restart.json` under the stable data root and has no
receipt-output argument.

`recovery-kit-live-smoke.sh` requires a signed Home or System session through
`ELASTOS_HOME_TOKEN`, a Cookie header, or a cookie jar. Export also requires a
fresh request-bound passkey token in
`ELASTOS_FRESH_PASSKEY_HOME_TOKEN`. Import into the same root is opt-in through
`ELASTOS_RECOVERY_KIT_IMPORT=1`.

`custody-harness-ci-smoke.sh` is the CI-safe machinery rehearsal for the
protected-content dKMS ceremony and custody harness: it builds the
`deploy/custody-host/` image, brings up a fresh throwaway instance of the
three-node compose harness, runs `protected-content-installed-e2e-proof.sh`'s
`provision` and `preflight` phases against a throwaway client identity,
asserts both receipt blocks report `ok: true`, then tears everything down
(restoring any already-running default harness project it had to stop first).
It proves the provisioning and preflight machinery only, not the live HTTP
journey or drill phases.

`deploy/custody-host/` builds that simulation-only container image and
same-host, container-per-node three-node compose harness; see its own README
for the explicit simulation boundary and the operator flow it supports.

## Subdirectories

- `build/`: build and staging helpers
- `fetch/`: asset and tool fetchers
- `fixtures/`: test-only proof fixtures
- `lib/`: shared shell and JavaScript helpers
- `system/`: installable operator-service files
- `dev/`: local development helpers outside the public command contract

## Placement rules

- Keep one canonical path per operation.
- Put directly invoked, reusable commands at the root.
- Put shared implementation in a named subdirectory.
- Keep installed mode explicit where a command supports both source and
  installed paths.
- Keep host-specific secrets and private maintenance commands outside the repo.

## Signing and publication ownership

Builders prepare inert inputs. The custodian signs their approved hashes. The
publication host imports that frozen output and serves it; it does not announce
it. See [VERSIONING.md](../docs/VERSIONING.md#publishing-a-release).
Each host has a separate account and role; the publication host receives the
public DID and signed files.

The key-free builder checks that the prepared Runtime's `--version` prints
exactly `elastos VERSION` on stdout with empty stderr, and records its hash.
Candidate executables run in that builder account; the custodian receives
inert files. Pin the publication Runtime and provider paths and hashes, and
retain the reviewed source checkout that supplies the Runtime's compiled-in
helper paths. Use isolated builder and publication accounts with approved
state; unsigned preparation also imports files through its selected provider.

For a canary on Apple silicon, the builder can prepare one qualified native
input. Run this from the reviewed source checkout, with an absent output
directory outside it:

```sh
ELASTOS_PUBLISHER_GATEWAY=https://staging.example.invalid \
ELASTOS_PUBLISHER_NODE_ID=HOLDER_NODE_ID \
ELASTOS_SOURCE_CONNECT_TICKET=HOLDER_TICKET \
/path/to/reviewed-elastos publish-release --version VERSION --channel canary \
  --platform-input aarch64-darwin=/path/to/native-input \
  --preview-platform aarch64-darwin --prepare-only /path/to/unsigned-input \
  --publisher-did DID --ipfs-provider-bin /path/to/qualified-ipfs-provider
```

Select the staging HTTPS origin and public holder node/ticket before preparation;
the same installer stamps remain fixed across the two staged versions.

The resulting `signing-input.json` binds the source commit/tree, artifact
hashes, sizes and CIDs, release data and public installer stamps. The operator
records its SHA-256 in a protected policy outside the input directory. That
policy also pins the repository, exact source `commit` and `tree`, version,
channel, public DID, file/total quotas and the trusted Python, OpenSSL and
signer tool paths and SHA-256 hashes. For `canary`, the policy pins the current
remote `develop` head in `develop_oid`; the source commit must be that head or
its ancestor. For `stable` and `jetson-test`, the policy pins `tag` as `vVERSION`
and `tag_oid` as the remote tag object; the tagged source commit must belong to
`main`. The signer selects these fixed repository branches from the channel.

Before approving a canary policy, the operator verifies the exact candidate
commit/tree against its merged pull request on GitHub and the successful
required checks for that commit. The installed signer also comes from a
reviewed commit merged into `develop` with successful required checks. The
operator verifies its source commit/tree and installed file hash, and records
the canonical GitHub pull request and check links in the owning issue. The
protected policy approval carries this review and CI acceptance; the signer's
GitHub source checks prove commit/tree identity and branch membership. Anders
approves each canary signing and publication. The operator alone
sets the protected Ed25519 PEM key path. Use the signer from its installed,
reviewed path, with a clean environment and its pinned Python:

```sh
env -i /path/to/pinned-python -I -S /path/to/installed-release-signer.py \
  --policy /path/to/operator-policy.json --input-root /path/to/unsigned-input \
  --output-root /path/to/new-signed-set
```

The tool verifies the pinned remote `develop` head and candidate ancestry for
canary, or the approved remote version tag and `main` ancestry for the other
channels. A moved `develop` head requires a fresh operator policy approval.
The tool reads candidate files as data and asks the operator to confirm the
complete public signer DID before signing. It fetches the installer template
from that approved tree. Its frozen installer keeps `HEAD_CID` empty to avoid
a hash cycle; the signed head binds the final
release CID and installer hash. Carrier holder identity and ticket remain
public transport inputs, separate from the signer identity.

On the publication host, inspect the frozen set before committing it:

```sh
/path/to/reviewed-elastos publish-release --version VERSION --channel canary \
  --signed-publication /path/to/new-signed-set --publisher-did DID --dry-run
/path/to/reviewed-elastos publish-release --version VERSION --channel canary \
  --signed-publication /path/to/new-signed-set --publisher-did DID \
  --ipfs-provider-bin /path/to/qualified-ipfs-provider
```

A saved pin change requires `--allow-signer-rotation` and confirmation of the
complete public DID. Include the flag in both the dry-run and import commands
when the saved pin changes. Runtime verifies signatures, exact artifact hashes and
imported CIDs, promotes the public pin and files before the head, and restores
the prior set if promotion fails. A complete rollback removes the backup links
and commits the original signed head again, so cached HTTP reads can admit the
restored set. A restoration, cleanup or final-head failure reports incomplete
recovery and retains the attempt directory for operator inspection. A committed
set can be retried to finish its ledger. The HTTP
gateway serves each release file when the saved public pin and complete signed
set agree; `install.sh` stays byte-identical across gateway hosts.
