# ElastOS 0.7.1 execution handover

Session closeout requested by Anders on 2026-09-10 after 49% of the weekly
allowance was used. Resume only when Anders asks. The full five-journey release
mission remains open; this is a handover, not release acceptance.

## Resume here

Read this document, the single Now queue in [TASKS.md](../../TASKS.md),
[state.md](../../state.md), [PRINCIPLES.md](../../PRINCIPLES.md), and the approved
[Notion plan](https://app.notion.com/p/wauio/ElastOS-0-7-1-release-plan-five-user-journeys-from-start-to-stop-3d6b682adcca81948f78d12abcd677b9).
Notion owns D1–D6 and required acceptance. TASKS owns next work; state owns
verified facts. Private process, path, proof and inventory details are in the
local operator ledger and `.git/development-loop-current.md`.

Use `feat/0.7.1-website-execution`, based on the user-selected
`origin/upstream/0.7.1-dev` at `6c61c990`. Preserve the integration and Browser
donors. Before edits, recheck HEAD/tree, dirt, fetched divergence and target
receipts. Local source is ahead of installed artifacts. A new source commit
alone does not require rebuilding every component.

## Delivered and verified

- Storefront design/content follows the supplied reference. Its launch path is
  `/home/`. The private seed serves product candidate `d790a48e`.
- Real signed Mac installer, default component setup, account creation,
  sign-out/sign-in, System entry and Profile-inclusive Recovery export pass.
  Anders also installed the preview successfully.
- Linux seed source/installed/served parity and actual Recovery export pass.
  A clean seed Home is available separately from automated test identities.
- Carrier transfer cleanup is committed as `4e62b6db`; six focused lifecycle
  tests pass. It awaits the combined installed test build.
- Recovery export smoke `49a62db1` uses the loaded foreground System window.
  Earlier failures were retained: static controls and restored covered windows
  made the test click the wrong surface. The final actual export passes.
- Public deployment has a concrete staged package and preservation plan.
  Public live remains unchanged; deployment requires explicit approval.

## Current repairs and acceptance limits

The kit-first recovery source opens the file picker when Recover is selected,
then performs required passkey setup and imports through Runtime. Profile DID
and exact name are checked by recovery tests and the Home summary. File/password
state stays in memory. Wrong encrypted passwords are checked by the server after
passkey setup. A reload clears this memory; sign-in/System recovery is still the
resumption path and needs a coherent follow-up. Installed recovery acceptance
remains open. The journey workbook's AUTH-01 wording also needs reconciliation
with the accepted Profile-first Create and kit-first Recover behavior; retain
its actual pending verdict.

Gateway shutdown must close active connections before releasing data-root
ownership. A timeout that merely abandons waiting is insufficient. The final
source check must prove connection EOF/reset, child reaping and safe restart.
Media tools are downloaded twice because the named installation-state check
mistakes a tool archive for a capsule requiring `capsule.json`; preserve archive
CID/checksum validation when correcting that classification.

Closeout source acceptance: recovery `67572db1` passes enrollment edge cases,
six viewport checks, encrypted fresh-machine and real-enrollment recovery
checks with exact Profile DID/name and Home summary assertions. Media-tool
reuse `f8824ee4` passes seven setup/cache checks. Gateway connection closure
passes five isolated tests in 2.03 seconds, including EOF/reset, child reaping
and safe reuse. The combined run's fixture failure came from concurrent PATH
mutation by setup tests; isolate environment-mutating tests in future runs.
All three repairs passed source review. They are not installed merely
because source checks pass. Preserve both of Anders's Mac test Homes and keys.

## Remaining goals, in priority order

1. **Finish J1:** verify the current recovery/shutdown/install repairs together;
   prove restored Profile/name, returning sign-in, Desktop/Terminal and shared
   app behavior. Complete remaining setup, save-conflict and window work.
2. **Deliver the public preview:** approve and deploy the reviewed storefront
   and seed Runtime, preserve existing accounts, then verify public artifacts.
   Public signed installer delivery remains a separate gate.
3. **One current Home assistant and working local AI (J3):** Anders reports two
   Home assistants, one outdated, and a model that does not load. Reproduce on
   the actual installed path, map both surfaces to their source/donor owners,
   preserve drafts/data, and converge on the intended single experience.
   Prove model Get, local reply, stop, restart/reuse and removal through the
   canonical typed Model and Content/Carrier contracts.
4. **Working Browser (J4):** Anders reports that Browser does not load. Start
   with that installed failure, then integrate the canonical Browser work and
   prove the agreed Mac-local and Linux Home/Exit with Mac Engine placements.
   Retain reload, media, lifecycle and full required qualification gates.
5. **Update and protected content (J2/J5):** prove ordinary 0.7.0 first-hop
   update and interruption/data preservation; review PR60 → PR59 and required
   PR62 work; complete the controlled video mint/list/buy/play/close journey
   and arrange the required external cryptographic review.
6. **Final release:** reconcile donor history and dirty branches; run combined
   human/target acceptance; freeze source, assemble/sign the compatible three-
   platform installer and binaries, then publish only with required approvals.

Jetson is deferred. Optional hosted/Codex providers, broader placements and
later operator work retain the Notion labels. None replaces required local AI,
Browser, shared Home, update or protected-video acceptance.

## Branch and entropy discipline

The closeout inventory records every branch's commit/tree, worktree dirt,
protecting refs and preservation decision. Inventory: 75 local branches and 12 worktrees; four were dirty at the
initial closeout scan. Many branches still hold unmerged
work; the detached Browser build and historical backup refs need explicit
reconciliation. No branch or user state was deleted during closeout.

Before integrating a donor, map its unique commits and dirty changes to one
owning journey. Preserve semantic conflict resolutions, especially provider
bounds/lifecycle, Documents `if_revision`, Profile authority and the single
components cutover. Compare history as well as tree bytes before retiring a
ref. The next session must not declare the whole repository clean based only on
the website worktree. Keep at least 10% free disk space.

Runtime owns authority, networking and lifecycle; signed Profile authority owns
identity/name; Content/Carrier owns model delivery. Remove obsolete surfaces
only after proving their current replacements and preserving user state.

## Lessons to apply next session

- The loop produced useful artifacts but spent too much effort on orchestration,
  repeated status and broad tests before a usable milestone. Start with one
  observable user failure and one short installed test.
- Validate the harness: wait for loaded controls and use the foreground window.
  Capture bounded UI/network diagnostics on the first failure, then change the
  experiment rather than repeat a long run without new evidence.
- Keep one writer per file and one test/build owner per cache or target. Batch
  related final checks; avoid compile contention and repeated full assembly.
- Freeze preview artifacts during testing. Record source, installed and human
  verdicts separately, and rebuild only affected outputs after accepted fixes.
- Keep one compact current checkpoint with links to receipts. Update milestone
  summaries after meaningful results; avoid appending contradictory snapshots.
- Use a short practical scope and report passed/failed/pending work. Report
  progress percentages only against an explicit denominator. Agree a usage or
  time checkpoint before the next sustained run.

Fresh origin fetch: dev remains `6c61c990`; protected-content follow-up moved
from `decab1f5` to `06179578`. Recheck PR62 against that newer source.
