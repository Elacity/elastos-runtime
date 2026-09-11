# ElastOS 0.7.1 execution handover

The original combined automated checkpoint passes on Mac ARM64 and Linux x86_64.
User-review follow-up `1e320578` repairs the restored account label, Security
spacing and session collisions between localhost Homes. Both targets pass the
shared-browser reload/refresh/sign-out check and show the exact restored name.
[The checkpoint report](2026-09-11-first-checkpoint.md) separates current
follow-up proof from reused lifecycle/media receipts. The two isolated Homes
preserve the user's restored data and retain the full AUTH-01 human gate. Anders
accepted the visible repairs, published the branch at `00003b6f`, and resumed
C3/J3 on that branch. Recurring monitoring stays paused. Full J1–J5 and D1–D6
acceptance remains intact; the existing branch replaces the proposed extra J1/J3
working branch names under the user's latest instruction.

## Resume here

Read this document, the single Now queue in [TASKS.md](../../TASKS.md),
[state.md](../../state.md), [PRINCIPLES.md](../../PRINCIPLES.md), and the approved
[Notion plan](https://app.notion.com/p/wauio/ElastOS-0-7-1-release-plan-five-user-journeys-from-start-to-stop-3d6b682adcca81948f78d12abcd677b9).
Notion owns D1–D6 and required acceptance. TASKS owns next work; state owns
verified facts. Private process, path, proof and inventory details are in the
local operator ledger and current checkpoint. Resolve their directory with
`git rev-parse --path-format=absolute --git-common-dir`; read
`development-loop-current.md` there. A linked worktree's `.git` is a file.

Use `feat/0.7.1-website-execution`, based on the user-selected
`origin/upstream/0.7.1-dev` at `6c61c990`. Preserve the integration and Browser
donors. Before edits, recheck HEAD/tree, dirt, fetched divergence and target
receipts. Human preview Homes remain `1e320578`; the separate Mac J3 Home has
accepted Runtime `fbf1a4b0` Qwen proof. The approved one-Assistant source now passes
preservation and rendered checks. Install its frozen candidate next, as recorded
in [the convergence check](2026-09-11-assistant-convergence.md). The affected
Runtime build has passed; reuse providers, engine and model data.

The installed checkpoint code is `1e320578b6b8a77753c4b5ef8fab51c12ea3d9e2`, tree
`44b08f00907c02dd3fe3bb2d3ed8e79e1bb76788`, 54 ahead / zero behind dev `6c61c990`.
Closeout documentation follows that candidate. The default checkout contains
unrelated dirty donor work; use the website checkout explicitly.

One owner built the Runtime once per platform for the revised candidate and
reused unchanged verified artifacts. The old pinned assembly helper was reviewed
and left unused. Official installation receipts were refreshed after manifest
changes. Automated test gateways are stopped; only isolated human-test Homes
remain for AUTH-01. Existing user previews and public stage retain `d790a48e`.

The full model donor `6972e165` is merged by `b318bfda`. Sash's shelf history
is reconciled by `37c82d4f` after full behavior comparison. Irzhy
foundation `617796a9` (PR60, which replaced PR57) is merged by `dd21d8bd`. Further
intake is frozen for one installed Qwen journey in a fresh test-owned Mac Home. Reuse the existing signed model package and engine. Prove
catalog visibility and normal cancel before a large transfer, then preparation,
selection, reply, stop and restart/reuse. Carrier cold delivery retains its
separate Required gate. Existing user Homes stay preserved.

The [preservation check](2026-09-11-integration-preservation.md) accounts for all
13 Irzhy and Sash PRs created or updated in September. It records original
history, adopted behavior, explicit differences and remaining proof. Adapted
URUX/UIUX already entered through Irzhy reconstruction; compare remaining UI
differences against that baseline. Irzhy authority cutover and Required follow-up
work retain their dependency order. Keep both Assistant workspace stores intact;
one visible Assistant requires full history and draft preservation. Integrated
UI and full model acceptance remain pending. Installed Mac Runtime `fbf1a4b0`
now passes exact Qwen admission, cold startup, selection, an actual typed reply
and saved conversation. Reply, draft and exact CID/offer survive reload with zero
implicit dispatches. Stop returns an honest unknown outcome, as permitted by J3
Step 5; confirmed backend cancellation remains separate. The native adapter
intentionally records unknown when stream closure cannot prove backend stop.
Owned shutdown closes all 29 processes including the engine, a held HTTP connection
and the listener. The same installed Home restarts, produces a second real reply
and restores the original draft. All six package files, protected files and prior
message hashes match; measured reuse receives zero Bitswap payload and makes zero
Content mutations. Independent review accepts this bounded Mac milestone. Current manifests and receipts bind all changed artifacts;
providers, engine and the admitted package are reused. The model menu repair `8f28b6e3` passes installed refresh, placement, normal close
and workspace-preservation checks. A stale copied archive identity was corrected
in source-install metadata; the bounded stamper repair prevents recurrence.
The original URUX full-merge proposal is superseded: the UIUX candidate
`8b547590` entered through reconstruction `7dd1780b` and Irzhy merge `985fdffc`,
followed by newer Sash dock/lock changes. Next, verify concrete remaining UI
differences and preserve all Assistant workspaces before consolidation. Preserve the
legacy `homeBrowserState.session.agent` object as well as both current workspace
stores. Repair concurrent saves and durable history limits on the combined clients.
The 79-conflict preview remains historical; no actual URUX merge was started.
Linux needs additional disk headroom and a reviewed startup profile. The private
checkpoint owns current processes and receipts; user Homes remain preserved.
Public staging needs exact-candidate review and approval. Final installer stamping
and signing follow C5 source freeze.

## Earlier preview evidence

- Storefront design/content follows the supplied reference. Its launch path is
  `/home/`. The private seed serves product candidate `d790a48e`.
- Real signed Mac installer, default component setup, account creation,
  sign-out/sign-in, System entry and Profile-inclusive Recovery export pass.
  Anders also installed the preview successfully.
- Linux seed source/installed/served parity and actual Recovery export pass.
  A clean seed Home is available separately from automated test identities.
- Carrier transfer cleanup is committed as `4e62b6db`; six focused lifecycle
  tests pass. Installed success/error/timeout cleanup and endpoint reuse now pass; exact
  internal cases retain their separate source evidence in the checkpoint report.
- Recovery export smoke `49a62db1` uses the loaded foreground System window.
  Earlier failures were retained: static controls and restored covered windows
  made the test click the wrong surface. The final actual export passes.
- Public deployment has a concrete staged package and preservation plan.
  Public live remains unchanged; deployment requires explicit approval.

## Current repairs and acceptance limits

Recover opens kit selection first. Both installed targets restore the original
Profile DID/name, preserve existing identity data after a wrong password, and
complete browser retry and reload/sign-in/System continuation. `a45164cf` fixes
completed-kit retry after a fresh sign-in while retaining strict token binding.
AUTH-01 is corrected and remains pending final user acceptance.

Full Home shutdown exposed detached managed children after the earlier API-only
pass. `f21d285e` repairs ownership and awaited cleanup. Both targets now close
three held connections, stop all owned descendants, remove coordinates, release
the port and restart the same Home. Full Home setup twice, media integrity and
installed Carrier cleanup/reuse pass. The report retains first failures, source
proof boundaries and exact artifact hashes. Automated acceptance passes; the
wider J1 app matrix and human qualification remain separate.

## Remaining acceptance and goals

The active C3/J3 step above and TASKS.md own execution order. These remaining
obligations retain their release gates while that source intake proceeds.

1. **Finish J1:** finish the checkpoint’s user-operated passkey proof;
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
protecting refs and preservation decision. The follow-up review reduced 75 local
branches to 70 by removing five fully preserved branch names. Their tip commits
and reflog revisions remain reachable from retained refs, with an exact restoration
map in the private cleanup receipt. Another 24 merged-tip candidates retain
historical reflog work and need review before removal.

All 12 worktrees remain. The active website checkout is clean after the checkpoint closeout commit; the
root documentation, Browser maturity and Home URUX worktrees contain preserved
dirty work. The detached Browser build is contained in `fix/browser-maturity`.
Three historical backup refs and the clean review/build worktrees retain their
ledger owner and cleanup conditions. Broader donor integration and cleanup remain
open. Branch count alone is not a reason to merge incompatible work.

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
