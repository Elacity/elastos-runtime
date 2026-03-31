# Runtime Versioning

Strict release versioning policy for `elastos-runtime`.

This borrows the useful parts of the Elacity SDK versioning standard while adapting them to this repo's reality:

- one coordinated runtime release train, not independently versioned packages
- one checked publish ceremony, not auto-publish on every merge
- one root changelog in [`elastos/CHANGELOG.md`](../elastos/CHANGELOG.md)

This repo currently has three distinct version layers. They should not be read as if they mean the same thing:

1. **Core runtime release train**
   - the coordinated workspace line under [`elastos/Cargo.toml`](../elastos/Cargo.toml)
   - examples: `0.20.1-rc.4`, `0.20.1-rc.3`
   - this is the public release identity for the runtime/server crates

2. **Stamped public release version**
   - the publish/install/update version injected through `ELASTOS_RELEASE_VERSION`
   - this is what installed binaries, published providers, and published capsules should report to users
   - non-stamped source builds may append `-dev`

3. **Capsule-local package version**
   - many standalone capsules and helper tools still carry local package versions like `0.1.0`
   - these are package-development identities, not the coordinated public runtime release identity
   - if a published artifact is stamped, the stamped release version is the user-facing truth

Current rule:

- treat the coordinated runtime release train and the stamped public release as the product version
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
- `0.21.0-rc.4`
- `0.21.0-beta.3`

Legacy compatibility examples still accepted by the ceremony for now:

- `0.20.0-rc31`
- `0.20.0-beta3`

New releases should prefer dotted prerelease identifiers such as `-rc.31`, not `-rc31`.

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

This repo is still pre-release. Public runtime lines may continue to use `-rc.N` while the product is not ready for a stable `1.0.0` contract.

Current rule:

- unstable public candidate: `X.Y.Z-rc.N`
- stable release: `X.Y.Z`

The release channel and the version are related but not identical:

- channels decide where a release head points: `stable`, `canary`, `jetson-test`
- the version string describes the release artifact itself

Do not invent ad hoc suffixes. If a new prerelease class is needed, update this policy and the ceremony together.

## Source vs Published Identity

When reading logs, audits, or UI banners, use this interpretation:

- **Published install**
  - should show the stamped public release version, for example `0.20.1-rc.4`

- **Source build without publish stamping**
  - may show the coordinated workspace version with `-dev`
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

## Ceremony Requirements

Before publish:

1. version passes [`scripts/check-versioning.sh`](../scripts/check-versioning.sh)
2. version choice matches the actual contract change (`MAJOR` / `MINOR` / `PATCH`)
3. changelog and public docs are honest about the release
4. checked publish ceremony passes
5. remote proof passes before marking the published baseline

Current enforcement:

- `scripts/publish-ceremony.sh` validates the version string before any publish work
- `scripts/publish-release.sh` validates the version string before low-level publish logic
- the ceremony also stamps `ELASTOS_RELEASE_VERSION` into published runtime/provider/capsule builds so installed artifacts report the coordinated release line instead of raw local package metadata

## Tagging Recommendation

Use annotated repo tags for runtime releases:

```text
vX.Y.Z
vX.Y.Z-rc.N
```

This repo is a coordinated release train, so package-style per-component tags are less useful than one signed repo-level release tag per publish.
