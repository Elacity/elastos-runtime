# Home and model handover, 2026-09-09

This is the 2026-09-09 documentation closeout. Product execution and installed
work are paused at the user's request. Analyser reviewed this documentation
closeout and verified that the source-closeout automation is paused;
there is no active native goal.

## Resume here

First restore effective controls on the existing signed-in owner Home. Analyser
could read the page, but clicks failed; cancellation through normal Home controls
could not be proved. Preserve the current installation and user state while
resolving that external tool prerequisite. Then arrange a fresh shared-resource
window and review the exact Runtime-only update plan in the private operator
ledger. Its earlier time slot has expired.

Before installation or Retry:

1. Open and close System through the normal Home controls. Reading the
   accessibility tree alone does not prove that input works.
2. Recheck signed catalog expiry and trust, the current account and grant, and
   disk admission with at least 10% free space. An expired or changed input
   requires an explicit operator decision; preserve the existing catalog and
   credentials until that decision.
3. Match the reviewed source and build configuration to the retained artifact
   hash before reusing it. Verify the installed receipts and preservation set,
   then obtain the fresh resource slot and installed-action approval.

After admission and artifact/preservation checks, Analyser owns one bounded
normal-authority Retry. Stop on terminal failure, or cancel at first observed
weights progress or 60 seconds, whichever comes first. This is an observation
limit, not a hard byte limit. Verify settlement before any further attempt.
The historical preparation failure remains unknown; a new diagnostic result
does not retroactively establish its cause.

## Source and evidence

[Current source](../../state.md) records the pre-handover branch, commit, tree,
divergence and publication status. The diagnostic code and separate execution-plan commit
remain local. This handover changes documentation only.

The focused Rust test
`model_preparation_failure_phase_survives_cleanup_with_real_home_revalidation`
passed five cases: metadata integrity, first weights fetch, header validation,
real Home grant revocation after metadata, and failed drain retaining staging
and charge. `scripts/model-preparation-failure-smoke.mjs` passed shared-copy,
public-class and malformed/coercing-value checks. These tests use fixtures;
the installed failed attempt's returned bytes were not captured.

The clean-source Runtime-only release build passed with Rust 1.91.0,
`RUSTFLAGS='-D warnings'`, `CARGO_BUILD_JOBS=1` and
`cargo build --release --locked -p elastos-server --bin elastos` from
`$SOURCE_ROOT/elastos`, using `$SHARED_CARGO_TARGET`. The exact artifact hash and
size are in the [build checkpoint](../../state.md#diagnostic-build-and-handover-checkpoint).
This docs-only commit follows that build. Preserve its actual source provenance;
verify unchanged relevant build inputs/configuration and the artifact hash before
reuse rather than rebuilding for documentation alone.

The owner Home retains its earlier verified installation. The diagnostic binary
is built but was neither installed nor used for Retry.
See [current source and installed evidence](../../state.md#installed-candidate-proof)
for the full proof limits. `$OWNER_HOME`, `$DATA_DIR`, `$SOURCE_ROOT`,
`$SHARED_CARGO_TARGET` and recovery paths resolve only from the private ledger.
Keep accounts, Wallets, passkeys, profiles, content and browser tabs intact.

## Final scoped check

The diagnostic change preserves one preparation record, real Home grant checks,
Content/provider ownership and bounded public error classes. Its focused tests
and retained build proof cover that change. This closeout is not a fresh audit
of every older commit. The installed failure, consumer handoff and retention,
cold peer delivery and remote authority remain open in the canonical plan.

## Remaining ownership and scope

Use the nine ordered steps in
[Builder-only execution](../../TASKS.md#builder-only-execution) as the single queue.
The plan records the dependencies and exact proof needed for each step. Model and distribution
contracts remain in [Model Provider](../MODEL_PROVIDER.md) and
[Content capsule distribution](../CONTENT_CAPSULE_DISTRIBUTION.md).

Browser owns B01-B16 and its implementation. Irzhy owns dKMS and mint/buy/play.
Analyser owns independent review, resource coordination and installed/manual
acceptance; Anders owns final acceptance and publication. A new session starts
with inventory and the current approved boundary, not an automatic build,
installation, transfer or inference. Retain the verified recovery artifact until
diagnostic settlement and explicit cleanup acceptance; publisher-stage retention
has a separate owner decision. The private ledger contains the exact commands
and paths. No new rollback or user-data copy was created during this closeout.
