# Runtime Versioning

Strict release versioning policy for `elastos-runtime`.

This borrows the useful parts of the Elacity SDK versioning standard while adapting them to this repo's reality:

- one coordinated runtime release train, not independently versioned packages
- one checked publish ceremony, not auto-publish on every merge
- one root changelog in [`elastos/CHANGELOG.md`](../elastos/CHANGELOG.md)

This repo currently has three distinct version layers. They should not be read as if they mean the same thing:

1. **Core runtime release train**
   - the coordinated workspace line under [`elastos/Cargo.toml`](../elastos/Cargo.toml)
   - read its version from the workspace manifest; release status belongs in the release issue on GitHub
   - the workspace version is source package metadata; the signed public
     release carries the stamped version

2. **Stamped public release version**
   - the publish/install/update version injected through `ELASTOS_RELEASE_VERSION`
   - this is what installed Runtime binaries and published app capsules report to users
   - non-stamped source builds may append `-dev`
   - providers do not read it: they report their crate version, so a new release
     version alone no longer changes provider bytes (dependencies and toolchain still
     can); the Runtime accepts a provider by its protocol version and request/response
     schemas, never by its reported version

3. **Capsule-local package version**
   - many standalone capsules and helper tools still carry local package versions like `0.1.0`
   - these are package-development identities, not the coordinated public runtime release identity
   - if a published artifact is stamped, the stamped release version is the user-facing truth

Current rule:

- use the stamped identity for the signed public release; the workspace
  version alone does not identify the installed product
- treat capsule-local `0.1.0` values as local package metadata unless and until the repo intentionally unifies them
- do not present capsule-local `0.1.0` values as if they supersede or contradict a stamped public release

Reference inspiration:

- Elacity SDK versioning standard: <https://elacity.gitbook.io/elacity-sdks/versioning>
- Semantic Versioning 2.0.0: <https://semver.org/>
- Conventional Commits: <https://www.conventionalcommits.org/>

## Version Scheme

ElastOS runtime releases use SemVer:

```text
MAJOR.MINOR.PATCH[-prerelease]
```

Preferred examples:

- `1.0.0`
- `1.2.3-alpha.1`
- `1.2.3-rc.1`
- `1.2.3-beta.3`

Rejected examples:

- `1.2.3-alpha1`
- `1.2.3-rc31`
- `1.2.3-beta3`

Use dotted prerelease identifiers such as `-rc.31`, not `-rc31`.

## Meaning

- `MAJOR`
  Breaking public contract changes.
  Examples:
  - breaking CLI behavior or command names
  - breaking install/update contract changes
  - breaking rooted namespace changes such as `localhost://...` contract shifts
  - breaking capability or release-manifest semantics

- `MINOR`
  Backward-compatible new capability.
  Examples:
  - a new user or operator command
  - a new first-party capsule or provider
  - a new additive site/share/webspace workflow
  - a new platform tier or artifact in a compatible release contract

- `PATCH`
  Backward-compatible fixes and hardening.
  Examples:
  - bug fixes
  - proof or ceremony hardening
  - install/update reliability fixes
  - public-surface coherence fixes

## Pre-release Policy

Use `-alpha.N` for a signed test release published while a version is still
being built; the 0.8.0 train ships `0.8.0-alpha.N`. Use `-rc.N` for a signed
candidate that is published before its final version.
A monthly source release (a repository tag; signed releases over Carrier are
the only binary distribution) uses plain `X.Y.Z`; its notes state what is not
ready for users.
The release issue on GitHub records the active release and its evidence.

Current rule:

- test release: `X.Y.Z-alpha.N`
- unstable public candidate: `X.Y.Z-rc.N`
- stable release: `X.Y.Z`

The release channel and the version are related but not identical:

- channels decide where a release head points: `stable`, `canary`, `jetson-test`
- the version string describes the release artifact itself

Do not invent ad hoc suffixes. If a new prerelease class is needed, update this policy and the checked publish flow together.

## Source vs Published Identity

When reading logs, audits, or UI banners, use this interpretation:

- **Published install**
  - should show the stamped public release version, for example `1.2.3`

- **Source build without publish stamping**
  - may show the coordinated workspace version with `-dev`
  - for example, workspace version `1.2.3` becomes `1.2.3-dev`
  - this means “current source build”, not “published release”

- **Capsule manifest examples or local package metadata**
  - may still show `0.1.0`
  - this is not a sign that the public runtime reverted to `0.1.0`

The repo should avoid hiding this distinction. If an audit or user-facing surface can only display one version string, prefer the stamped public release version when available.

## Commit Discipline

Commit messages should follow Conventional Commits so release intent is legible:

- `feat:` -> usually `MINOR`
- `fix:` -> usually `PATCH`
- `feat!:` or `BREAKING CHANGE:` -> `MAJOR`
- `docs:`, `test:`, `chore:`, `ci:` -> no automatic bump by themselves

Unlike the Elacity SDK monorepo, this repo does not currently derive versions automatically from commit history. The operator still chooses the next version intentionally during publish. The commit convention is used here for clarity, reviewability, and changelog quality.

## Changelog Policy

This repo keeps one coordinated release changelog:

- [`elastos/CHANGELOG.md`](../elastos/CHANGELOG.md)

That matches the coordinated runtime release train better than per-crate changelogs. A publish is not complete unless the changelog and public status story are honest about what changed.

## Publishing a release

A release moves through three places: GitHub builds it without keys, the
operator Mac prepares and signs it, and the seed imports and serves it. Choose
the version by the contract change (see Meaning) and update the changelog first;
`scripts/publish-release.sh` checks the version format with
[`scripts/check-versioning.sh`](../scripts/check-versioning.sh).

A new model catalogue is produced, signed and pinned before release preparation, with the same key.
`components.json` carries no `model_catalog` until then: the release refuses a pin without `model-catalog.json`.
(1) Prepare the pinned Kubo with `scripts/seed-kubo-cache.sh CACHE KUBO_DATA PLATFORM`. For each model, run
`node scripts/ci-model-package.mjs produce FIXTURE OUT DID KUBO_DATA` with its pinned fixture (`scripts/pinned-smollm2-fixture.json`,
`scripts/pinned-qwen2.5-1.5b-fixture.json`); it downloads and checks the pinned weights, builds `OUT/package`, prints its CID and
byte size and writes `OUT/entry.json`. Then `node scripts/ci-model-package.mjs catalog CAT DID OUT1/entry.json OUT2/entry.json`
writes the unsigned `CAT/payload.json` with the entries in that order (1 to 8), after the signer's own catalogue check accepts it.
(2) Sign: `release-signer.py --policy POLICY --input-root DIR --model-catalog CAT/payload.json --output-root NEW_DIR`.
The policy approves the payload by `model_catalog_sha256`; the operator types the DID. The signer refuses a payload the Runtime would refuse,
writes `model-catalog.json` and prints its CID.
(3) Pin each `OUT/package` in the seed's Kubo with `ipfs add` and the `KUBO_ADD_FLAGS` from `scripts/ci-model-package.mjs`; each printed CID must equal its package CID.
(4) Put `model-catalog.json` at the repository root and add `model_catalog` to `components.json`: `head_cid` (the signer's CID),
`publisher_dids` (the signing DID) and `local_use` (`max_cache_bytes` 8 GiB, `max_model_memory_bytes` 4 GiB).
Keep that head stable across releases: a new head means every admitted model needs Use again.

The Community network travels the same way. The custodian signs its initial profile with
`release-signer.py --operation community-profile` ([COLLABORATION_NETWORK_PROFILE.md](COLLABORATION_NETWORK_PROFILE.md#initial-production-profile)),
which writes `collaboration-network-release-v1.json` and `collaboration-network-release-pin-v1.json`. Put the first at the
repository root and merge the second's `collaboration_network` object into `components.json` through a reviewed pull request,
then build the package. At prepare, set `RELEASE_COMMUNITY_DIR` to that signed output directory (canonical, owner-only, those
two files only). Without it the release names no network, and prepare refuses a source that pins one.

Before preparing a release, the operator creates an approval file in a protected
directory outside the candidate inputs and `RELEASE_WORK`. Set `RELEASE_APPROVAL`
to its canonical absolute path. The directory and file belong to the operator
or root and permit writes only by their owner. This file is an independent
approval: preparation reads it and never creates it from downloaded inputs,
the package run or the publisher's current HTTP response.

The approval has these fields:

```json
{
  "schema": "elastos.release.approval/v1",
  "repository": "Elacity/elastos-runtime",
  "source": {"commit": "APPROVED_SOURCE_COMMIT", "tree": "APPROVED_SOURCE_TREE"},
  "workflow": {
    "commit": "APPROVED_BUILDER_COMMIT",
    "tree": "APPROVED_BUILDER_TREE",
    "ref": "refs/heads/develop"
  },
  "delivery": {
    "MAINTAINER_DID": "APPROVED_SIGNING_DID",
    "PUBLISHER_GATEWAY": "https://APPROVED_PUBLISHER_ORIGIN",
    "PUBLISHER_NODE_ID": "APPROVED_CARRIER_NODE_ID",
    "SOURCE_CONNECT_TICKET": "APPROVED_CARRIER_TICKET",
    "IPNS_NAME": "APPROVED_IPNS_NAME"
  },
  "gh": {"path": "/ABSOLUTE/PATH/TO/INSTALLED/GH", "sha256": "APPROVED_GH_SHA256"}
}
```

Replace the descriptive values with independently verified identities. Source H
and builder W are full Git commit/tree IDs and may differ. Approve W's complete
workflow source, its helper code and pinned actions, rather than just its YAML
filename. The producer ref is `refs/heads/develop`. Approve
H through the ordinary exact-source CI acceptance flow before preparation.
The independent source approval is deliberate key-owner trust; direct signer
calls retain this operator boundary. Prepare and seed-upgrade enforce the CI gate.
Approve the publisher tuple from the existing operator trust record. The Carrier node is
its 64-character lower-case transport ID; every endpoint in the base32 JSON ticket
has that same ID. A new ticket or delivery identity requires a new approval.
The IPNS field can be empty when the approved delivery uses Carrier alone.
The installed `gh` executable has a canonical path outside candidate inputs and
a SHA-256 pin. It supports the attestation verification flags used by the signer.

1. **Build.** Run the `Release package` workflow from `develop`
   ([`.github/workflows/release-package.yml`](../.github/workflows/release-package.yml))
   with the source commit, the install version N and the update version N+1.
   Supply the qualified common ARM64 Browser archive's CID, independent SHA-256
   and exact byte size. The operator builds that guest outside CI from the
   committed Debian and Python locks, packages it once, and retains it in the
   publisher artifact store under its canonical CID filename. Configure the
   repository variables `BROWSER_IMAGE_PUBLISHER_DID` and
   `BROWSER_IMAGE_PUBLISHER_NODE_ID`; an optional
   `BROWSER_IMAGE_CONNECT_TICKET` secret supplies the direct bootstrap route.
   The candidate Runtime downloads this input with `source fetch-file` through
   the existing Carrier file stream. It checks the pinned peer, length and hash
   before image admission. Mac and Linux ARM64 consume the same archive; x86-64
   selects a remote Engine. Guest images remain outside CI builds.
   The source is the approved commit on `develop`. The run builds native pairs for
   `aarch64-darwin`, `x86_64-linux` and `aarch64-linux` (Jetson). N+1 reuses
   N's support bytes. Each pair uses fresh Cargo directories and checks versions,
   source and support parity; ARM64 also passes the Jetson compatibility gate.
   Build jobs have read-only permissions. Fresh hosted attester jobs execute W's
   helpers, read the downloaded payloads as inert data, and seal source H/tree and
   version from the original workflow inputs and admission output. The attesters
   hash every payload and publish small `release-proof-*` artifacts with receipts
   and bundles. Candidate scripts and executables stay in the build jobs.
   The job summary records each native artifact's ID and digest.
2. **Prepare.** On the operator Mac, from a clone of this repository, run
   `scripts/release-publish.sh prepare RUN_ID VERSION`. Before it downloads
   anything, it refuses unless `scripts/update-hop-previous-release.json` at
   source H names the release head the origin serves now (its `head_cid`), so
   H's CI update journey started from the release users run; step 5 moves that
   pin. It also refuses a source without `scripts/update-hop-oldest-release.json`
   (see [Skipped releases](#skipped-releases)). N+1 of a pair keeps N's source
   and so its pin; it passes once the public
   head is the head this Mac signed for N from the same run and that head
   follows the pin. It checks that the run
   succeeded from approved W/ref/event and that source H is on `develop`, downloads
   and verifies all three platform artifacts with their native name suffixes
   and certificate-authenticated proof attempts. A retained native artifact from an earlier attempt can use its original
   proof or a fresh proof from a later attempt in the same approved run. All selected
   artifacts bind the same H/tree; the latest overall run must succeed. This permits
   a Linux-only retry to preserve good Mac build bytes. The native name suffix
   is a selection label; GitHub's certificate authenticates the proof attempt.
   Each proof binds the selected artifact's immutable ID and ZIP digest.
   Prepare passes one
   `--platform-input PLATFORM=DIR` per platform. Prepare also requires successful
   CI at that exact source commit, from a `develop` push or `develop` merge group,
   with every required job completed successfully. For a Mac-only preview, run
   `scripts/release-publish.sh prepare RUN_ID VERSION aarch64-darwin`. The Mac
   coordinator requires `aarch64-darwin` in every selection; only a single
   platform uses `--preview-platform`. Prepare checks out the exact source as
   a worktree, takes Kubo from the Mac build, checks the public bootstrap against
   the independent delivery approval, copies the seed's
   current `publish-state.json` and runs `scripts/publish-release.sh
   --prepare-only` in an isolated data directory. It then binds the Community
   network: with `RELEASE_COMMUNITY_DIR`, the custodian's pin must equal the
   source pin and name the exact startup-file bytes, network ID and signer set
   the release carries, and the package's own `aarch64-darwin` Runtime must
   accept the file with `elastos collaboration-config verify`; the signer
   refuses a pin whose file, network ID or signer set differ.
3. **Sign.** `scripts/release-publish.sh policy VERSION SIGNER KEY OPENSSL`
   writes the exact signer policy (with the current `develop` head and the
   independent approval file's path and SHA-256) and prints the `release-signer.py`
   command. The installed signer verifies W/tree/ref and that W's
   `release-package.yml` equals `develop`'s, the GitHub-hosted certificate
   identity, event, run and exact proof attempt, then binds each receipt to its
   selected native artifact ID, digest and name suffix and the frozen
   native bytes. Every published file is attested bytes, except
   `components-PLATFORM.json`, which must equal the manifest the signer
   re-derives from the approved source `components.json` and each platform's
   attested `components.json`. It shows source, builder, publisher tuple and approval digest
   before the operator types the DID. The source admission gate still applies.
   A network, outage or rate-limit failure refuses with a retry message,
   distinct from a provenance refusal; the printed command passes the operator's
   `gh auth token` to avoid anonymous rate limits.
4. **Import.** `scripts/release-publish.sh seed VERSION SIGNED_DIR` prints the
   seed sequence for the seed's installed Runtime: copy the signed installer
   and manifests, rebuild the rest from the selected CI artifacts, verify every
   hash, stop the service, run the preflight, import, and start the service.
   The import sends no gossip. Older seed Runtimes still print `No running
   runtime found` after the commit; that is expected, and the sequence accepts
   only that error. The seed's Runtime admits only release files it knows: the
   first release that carries `collaboration-network-release-v1.json` needs a
   seed Runtime with Community network admission, so run `seed-upgrade` from a
   package that has it before this import; an older seed Runtime's preflight
   refuses the file before it imports anything. Releases from 0.8.0 list per-capsule records
   (`capsule_records`, each `elastos.capsule.record/v1` signed under its own
   domain `elastos.capsule.record.v1`). `release-signer.py` derives one record
   per released archive from the verified components and signs it; a record
   names the capsule's bytes and declared version, never the release version,
   so it stays byte-identical while the capsule is unchanged. Seed Runtimes
   before 0.8.0 accept these releases and do not check the records; a 0.8.0
   seed checks each record against its components row. Homes check records when
   a release lists them; the release after 0.8.0 makes them required.
5. **Pin for CI.** Run `python3 scripts/update-hop-compare.py pin-previous-release`
   and merge the updated `scripts/update-hop-previous-release.json` into `develop`.
   It checks the signed head and release the seed now serves. Every pull
   request's CI update journey then starts from this release, and the next
   `prepare` refuses a source without it.

**A final release** (`X.Y.Z`) uses the same steps and is published on
`stable`; prereleases are published on `canary`. The seed serves one head, and
a Home subscribed to `canary` also accepts a newer `stable` head from its
trusted signer, so one signed stable head reaches canary Homes in System, and
stable Homes and fresh installs through `install.sh`. A stable Home never
accepts a canary head, and running `install.sh` again moves a canary Home to
`stable`. Before step 1, merge the version change (`elastos/Cargo.toml` and the
changelog) into `develop`, merge the release pull request from `develop` into
`main`, and tag its `develop` commit `vX.Y.Z`. Build that commit in step 1.
Prepare refuses unless the tag names the source commit, `main` contains it and
has the same tree, `elastos/Cargo.toml` has that version and all three
platforms are selected. The policy pins the tag for the signer, which checks
again, before and after the DID is typed, that the tag names the source and
that `main` contains it with the same tree; the stable release needs a signer
copy installed from this source.

The seed only answers requests. It serves three Carrier ops: `release_head`
(the signed head), `content_fetch` (any object by CID, from its Kubo pins) and
`file` (a file by name, for older Homes and for parts without a CID). It keeps
every published release pinned, so Undo and older Homes keep working. It does
not push or notify anything. How Homes check and install is in
[INSTALL.md](INSTALL.md#how-updates-reach-your-home).

Upgrading the seed Runtime itself is rare. Use the same independent approval and run the workflow with
`seed_package` set, then `scripts/release-publish.sh seed-upgrade RUN_ID` prints
the steps only after the operator Mac verifies the approved producer, attested seed
payloads and successful exact-source CI. Those steps create `/opt/elastos` for the service user, check and install the
exact source at `/opt/elastos/release-source` (the seed Runtime runs its
release helpers from the path it was built in), install the Runtime and IPFS
provider, update the provider's pin in the installed `components.json`, verify
the provider and start the service. The seed needs glibc 2.39 or newer.

Both seed sequences check sudo and every path they write before stopping the
service, and once it is stopped an `EXIT` trap starts it again if any later
step fails, so the seed is never left down. `seed-upgrade` checks the producer and
exact-source CI through the same acceptance boundary as `prepare`. The printed
commands retain the approved payload hashes and source tree before installation.

Historical artifacts that lack fresh-job receipt attestations need a new run from
an approved builder containing this workflow and its complete helper closure.
Activate the workflow on the approved canonical ref and record its exact W/tree
before dispatch. Exercise receipt verification with an isolated disposable-key
fixture before production signing; local parser fixtures alone do not prove the
GitHub OIDC certificate and CLI integration. Install the updated signer and verifier before using
the new preparation flow; retain independent tool pins and the operator approval
under their existing access controls. Changing the workflow ref, source, ticket or
tools is an explicit new approval, rather than preparation-time discovery.

Seed details come from `RELEASE_SEED`, `RELEASE_SEED_DATA`,
`RELEASE_SEED_UNIT`, `RELEASE_SEED_STAGE`, `RELEASE_SEED_RUNTIME` and
`RELEASE_SEED_USER`, not from this repository. `RELEASE_SEED_DATA` is the
service's `$XDG_DATA_HOME/elastos` (absolute, ending in `/elastos`); the seed
scripts export its parent as `XDG_DATA_HOME`. Publish N first; after N is
accepted on a real install, prepare N+1 from the same run before its artifacts
expire (14 days), with N's work directory (and the `signed` directory the policy
command writes there) still in `RELEASE_WORK`. The printed seed commands carry
every value as one shell word; `seed` refuses a signed head whose `signer_did`
is not a `did:key` or whose `prev_head_cid` is not a canonical CID, and a first
publication (no `prev_head_cid`) may start without saved publish state.

## Skipped releases

A Home may skip any number of releases, as long as no release ships what the
oldest supported updater refuses. An updater refuses a whole signed release
whose components use a schema or a component kind (`strategy`) it does not
know. `scripts/update-hop-oldest-release.json` names the oldest release that
still updates in System, with the component kinds its signed releases ship.
The source gate (`update-hop-compare.py check-oldest-start`, through its test)
refuses a checkout whose `components.json` that release's updater cannot read
or install, and `scripts/release-publish.sh prepare` refuses a source without
the pin. When it and the previous pin name one release, CI's update journey
runs that start in full. To ship such a change anyway, decide that Homes on the
pinned release run install.sh once, say so in the release notes, and move the
pin with `python3 scripts/update-hop-compare.py pin-oldest-release` (it takes
the previous pin and reads that release's signed components from the origin).

A Home that still meets such a release does not change. From the release after
0.8.0-alpha.15, System and `elastos update` then say the install command is
needed once and print it, `curl -fsSL '<publisher gateway>/install.sh' | bash`,
with the URL built from the gateway the Home was installed from and quoted as
one shell word. Older updaters (alpha.15 and earlier) say the publisher's signed
release is not valid for this Home.

## Withdraw a bad release

A published release cannot be taken back: Homes refuse a lower version unless
the user runs Undo. Withdrawing a bad release therefore means publishing a
newer good one.

1. **Announce.** Post one comment on the release issue (#93) naming the bad
   version, its head CID, what goes wrong, and the last good version with its
   head CID. Tell affected users to stay on or return to the last good version
   until the replacement arrives.
2. **Publish N+2 from the last good source.** If version N is bad, its paired
   N+1 was built in the same run from the same source, so it is bad too. Run the
   [publishing steps](#publishing-a-release) with N+2 as the install version and
   the last good source commit, or a `develop` commit that reverts the bad change,
   as the source. Its changelog says it replaces N and why. In step 5, pin N+2.
3. **Effect.** The seed's head now names N+2. Homes on N update to it normally,
   because N+2 is newer. Homes that never installed N go straight to N+2, and new
   installs get N+2: nothing offers N any more.
4. **Undo for affected users.** A Home whose update to N failed to start has
   already restored its previous release. A Home that runs N but works badly can
   return to the last good release before N+2 is out with
   `elastos update --rollback-to <last good head CID>` (see
   [Undo an update](INSTALL.md#undo-an-update)). It then updates to N+2 normally.
   Add a comment on #93 when N+2 is published.

The seed keeps N pinned, like every published release; nothing is unpinned.
Only the head moves, so no Home is offered N again. N stays fetchable by its
CID, so an explicit `--head-cid` or `--rollback-to` that names it still works.

Decision (October 2026): for 0.8.0 the signed head carries no list of withdrawn
versions. Moving the head already stops every offer of N; a refusal list would
change the head schema and depends on who may sign it. Revisit with the head
freshness design (#157) and the publisher-keys statement (#213).

## Tagging Recommendation

Use annotated repo tags for runtime releases:

```text
vX.Y.Z
vX.Y.Z-rc.N
vX.Y.Z-alpha.N
```

This repo is a coordinated release train, so package-style per-component tags are less useful than one signed repo-level release tag per publish.
