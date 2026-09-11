# Integration preservation check

The user requested continued work on the published website branch, with one
Assistant experience and Qwen available through Marketplace as model content.
The September 11 correction makes preservation of Sash and Irzhy contributions
an explicit integration requirement. Keep original history where possible;
record every adapted behavior and every remaining feature before closing intake.
A successful Git merge establishes ancestry. Product checks establish behavior.

## Candidate and ownership

The starting source is `feat/0.7.1-website-execution@0962e7e9`, tree
`9467bcdbf7d6e60ee30f4d12dc9d4a43c0cf8593`. Its published prefix is `00003b6f`.
The integration donor is `feat/0.7.1-integration@6972e165`, tree
`8b32a72aec556a88248878f2021c2470ed5361af`. The coordinator owns the candidate,
source writes, builds and target tests. Independent reviews cover UI history,
merge preservation and the protected-content dependency chain.

Use a history-preserving integration merge, then reconcile the newer UI and
protected-content branches in dependency order. Existing donor worktrees and
uncommitted work remain protected. Source intake and installed acceptance have
separate verdicts. Public deployment and final release retain their existing gates.

## Behavior checks

| Required behavior | Source and disposition | Target and meaningful check | Expected result / current verdict |
| --- | --- | --- | --- |
| Recover selects a kit first, restores exact Profile DID/name, preserves data on failure and retry | Preserve current recovery/auth source and tests over older donor enrollment flow | Mac and Linux test-owned installed Homes; current checkpoint recovery journey after integration | Same accepted order and identity; integrated rerun pending |
| Two localhost Homes retain independent sessions | Preserve current authority-scoped cookie and header behavior | One browser context with two test-owned Homes; sign in, reload and sign out independently | Other Home stays signed in; integrated rerun pending |
| Shutdown closes owned connections/processes and permits restart | Preserve current gateway and managed Runtime lifecycle changes | Mac and Linux test-owned Homes; held connections, child ownership, lock/port release and restart | All owned resources settle; integrated rerun pending |
| Media reuse and Carrier cleanup remain intact | Preserve current integrity/reuse fixes and cleanup ownership; reconcile overlapping Irzhy source | Reuse current receipts for unchanged artifacts; narrow source regression and installed changed-artifact proof | Reuse with integrity validation and bounded cleanup; pending impact review |
| Sash Home layout and interactions survive intake | Compare original shelf/URUX branches and current extracted series, including documented reductions | Test-owned Mac Home; matched screenshots and interaction checks for shelf, dock, Agent Space, lock and recovery | Explicit account of every difference; pending |
| One public Assistant keeps user work | Both current Assistant and Home Agent workspaces and features require preservation | Both populated stores; full history, drafts, attachments, duplicate IDs, reload and repeat transition | Lossless access to both stores; consolidation pending |
| Marketplace discovers and loads Qwen content | Corrected donor catalog, Content verification, admission, model provider and composer selection | Fresh test-owned Mac Home; trusted catalog, model selection, prepare, reply, stop and reload/restart reuse | Exact content CID selects admitted offer; installed proof pending |
| Model failure preserves intent and releases ownership | Corrected donor cancellation, capacity, retention and diagnostics | Test-owned Mac Home; normal cancel/retry before costly transfer; first-failure diagnostics | Honest terminal/unknown state, preserved draft and eventual cleanup; pending |
| Irzhy contributions reach the integrated product | Installed-e2e proof, atomic cutover, then follow-up/Creator chain | Relevant source gates and J5 installed two-Runtime journey | Preserve complete dependency chain; source intake and J5 proof pending |

## Findings to retain

The Assistant and Home Agent workspace schemas differ. Direct conversion through
Home Agent's current serializer can truncate valid Assistant history. Preserve
both source objects until a versioned, idempotent transition retains all content,
both drafts and colliding IDs. A capsule rename alone does not satisfy this gate.

The current shelf code came through an adapted Sash series. Its recorded intake
removed Studio, Workbench and other controls whose Runtime contracts were absent.
Those features need explicit source and contract disposition; their removal does
not count as a completed consolidation. The original branches remain evidence.

Model content is passive signed data named by the complete bundle CID. Assistant
is its consumer; Runtime owns trust, admission, selection and lifecycle; the model
provider owns execution. Marketplace owns discovery and System owns inventory.
The existing donor model UI needs review for one-row assumptions and the ordinary
content-to-consumer handoff. Keep full J1–J5 and D1–D6 acceptance intact.

## Reviewed source order

| Source | Original history | Disposition |
| --- | --- | --- |
| Integration `6972e165` | 51 donor commits after shared development base | Full merge, preserving current kit-first recovery, scoped sessions and shutdown; source verification passed; installed acceptance pending |
| Sash shelf `923193bb` | Original four-commit series; adapted current series retains three equivalent patches and matching initial capsule tree | Merge ancestry after integration; retain later capsule-bound handoff checks |
| Sash URUX `5e546ef4` | 34 commits outside current ancestry; earlier local `d04c9df5` has protected dirty work | Merge latest committed source, reconcile layout and typed authority; retain current approved Create/Recover order |
| Irzhy installed proof `617796a9` | Hello, provider hosting, peer seeding, availability, proof harness | First protected-content merge; current media extraction is byte-identical but other features remain missing |
| Irzhy authority cutover `25ab205e` | Runtime authority cutover and unused API removal | Follow installed-proof merge; check removed APIs against combined model callers |
| Irzhy follow-up `06179578` | Lifecycle wiring, Creator and non-media objects | Follow authority cutover; reader/audio and full J5 proof remain open |

The UI comparison identifies original launcher-to-dock flight and reorder
motion, Agent Space Mission Control presentation, missing pager/switcher template
nodes, Viewer rail, System chrome and richer Assistant workflows. Restore each
with its matching behavior and authority checks. Existing current accessibility,
account, capsule attribution and workspace fixes remain required.

The integration workbook reconciliation restores donor-only AUTH-08, AUTH-09
and AUTH-10 rows with their pending installed verdicts. AUTH-01 retains the
approved recovery order and its human-proof boundary. Current source notes
separate the incoming model implementation from installed human Homes.

Source verification: catalog13, conditional storage3, Documents save7,
window policy1, preparation36, startup binding5, retention7 and retirement5 Rust
tests pass. Full Qwen/cold external fixtures remain ignored/pending in this
source run. Eighteen JS source/interaction smokes pass. Rendered model selection,
model management and System window policy pass after resolving the test harness's
missing Playwright dependency by reusing existing QA packages. Both Rust format
gates and diff whitespace pass. No installed artifact changed during this review.

Model-provider verification passes 204 unit and five real process tests. Two
external provider fixtures remain ignored. This adds source lifecycle evidence;
it does not close the installed Qwen journey.

Independent resolved-source review found a missing test group. The retained
recovery smoke now includes donor-derived Later persistence and real Save
cancellation/boolean acknowledgement, alongside all current kit-first and
Profile-create assertions. The combined smoke passes.

## Original Sash shelf ancestry

Independent comparison accepts recording `923193bb` with the current source
content preserved. Three original commits are patch-equivalent to the adopted
series. The fourth, `239c6cb7`, has the exact `home-agent` capsule tree of adopted
`77498557`: `c03166c4489f6773792c38c337f7ccb5e714f6ce`. Its component/profile,
source-home and release-packaging changes are also present. The three remaining
Carrier smoke hunks were added by `450db538`, with stronger installed-asset
verification. Thus this merge records already-integrated original history and
preserves later model and authority repairs. It adds no new UI claim.

This disposition is limited to the shelf branch. `923193bb` is not an ancestor
of URUX `5e546ef4`; URUX remains separate work with the feature checks above.
