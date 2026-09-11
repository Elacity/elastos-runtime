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
| Sash URUX `5e546ef4` | Original tip has separate ancestry; adapted UIUX candidate `8b547590` entered through Irzhy reconstruction `7dd1780b` and merge `985fdffc` | Existing adaptations and later Sash changes are already included. Review concrete remaining behavior differences; a full old-tip merge is superseded. Preserve the dirty donor and current recovery/model contracts. |
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
of URUX `5e546ef4`. Its separate ancestry does not establish missing UI work;
the adaptation and later Sash changes are accounted for below.

## September contributor PR coverage

The GitHub inventory includes every PR by `irzhywau` and `SashaMIT` created or
updated since September 1, read September 11. Nine were created this month and
four older PRs were updated. This is source accounting; combined installed
compatibility remains an independent gate.

| PR | Disposition |
| --- | --- |
| [51](https://github.com/Elacity/elastos-runtime/pull/51) | Development-to-main release PR; base source already in candidate; final release gate remains |
| [52](https://github.com/Elacity/elastos-runtime/pull/52) | Irzhy provisioning, original head already in ancestry |
| [54](https://github.com/Elacity/elastos-runtime/pull/54) | Sash first-run/chrome/Documents/Library/icons, original head already in ancestry |
| [55](https://github.com/Elacity/elastos-runtime/pull/55) | Sash shelf, complete original changes present; ancestry reconciled by37c82d4f |
| [57](https://github.com/Elacity/elastos-runtime/pull/57) / [60](https://github.com/Elacity/elastos-runtime/pull/60) | Same Irzhy source617796a9;57 closed in favor of60; foundation merged by `dd21d8bd` |
| [58](https://github.com/Elacity/elastos-runtime/pull/58) | Published integration prefix already in ancestry; later local donor merged byb318bfda |
| [59](https://github.com/Elacity/elastos-runtime/pull/59) | Authority cutover25ab205e, queued after foundation and installed checkpoint |
| [62](https://github.com/Elacity/elastos-runtime/pull/62) | Follow-up06179578, queued after59; Required repairs and Optional additions retain D2 |
| [38](https://github.com/Elacity/elastos-runtime/pull/38) | Older release updated this month; original head in base ancestry |
| [26](https://github.com/Elacity/elastos-runtime/pull/26) | Nonogram source adopted by1fd30b38; ROM/license/SVG byte-identical, viewer/storage/packaging and launch test retained. Old icon removal superseded by supported icon schema and assets. Original ancestry remains separate |
| [23](https://github.com/Elacity/elastos-runtime/pull/23) | Older URUX source; substantial adapted UI already entered through the UIUX/reconstruction chain, with newer Sash replacements. Only verified behavior gaps remain for reconciliation. |
| [19](https://github.com/Elacity/elastos-runtime/pull/19) | CI/setup behavior largely retained or reworked. Final plain-pipe CLI check retained. Original macOS Clippy/test steps now run in Linux jobs; named feature push triggers replaced by PR/manual/main/tag triggers. These differences remain explicit for CI policy review |

First-principles review narrowed the next operation: finish the active foundation
merge and its startup/lifecycle gates, then freeze further branch intake for one
installed Qwen journey. Carrier peer discovery supports Required cold delivery;
the current local model preparation still uses local Content. Later J5/URUX
source enters against a named missing behavior after this installed checkpoint.

## Foundation merge verification

The resolved `617796a9` source retains current model redirect handling and
provider shutdown ownership. Independent review found that an explicit occupied
Carrier bind address silently selected another address. The new regression test
reproduced this behavior; both explicit-bind error paths now return an error that
names the requested address. Default automatic selection retains its existing
behavior. A missing `anyhow::Context` import was repaired after the first compile
failure. The independent scoped review accepts these resolutions.

On Mac, the explicit bind test, six Carrier cleanup cases, peer-store startup,
three availability cases, 17 provider bridge cases, four bounded Content cases,
five model startup cases, six provider-host argument cases, unknown-provider
rejection and approval-authority purchase completion pass. Two startup fixtures
remain ignored. Provider-host missing-provisioning proof requires the dedicated
custody binary fixture: its first run failed with that missing input, before
product execution. Full provider-host and J5 installed proof remains pending.
Home entropy, format and protected-content static/harness checks pass. These
results establish source behavior; current human Homes retain their earlier
installed candidate.

## First installed model result

The Mac test candidate `dd21d8bd` uses newly built Runtime, model-provider and
ipfs-provider binaries, with unchanged inputs retained under their prior
receipts. Required manifests and installation receipts bind the source, hashes,
installed paths and running processes. Five served app assets match the installed
files. Existing human Homes retain `1e320578`; Linux has no new J3 result.

System and Marketplace both show the same signed Qwen package, verified publisher
and enabled Use control. Use, cancellation, Retry and a second cancellation pass
on the same test account. The first UI error was Playwright's service-worker
blocking script accessing a property denied to sandboxed frames. Removing that
injected harness script gives an error-free interaction run. The test Home closes
its owned processes and a held HTTP connection, then restarts successfully.

The retained Qwen package imports locally in 16.354 seconds with the expected
6,170,940,070-byte CAR hash and package CID. Network download is zero. Actual
Marketplace preparation then fails after 3,039,059 bytes with an inventory
`WouldBlock` error. This is a source defect: a short status snapshot can abort
progress. The regression test reproduces it. The bounded repair waits up to one
second for snapshot transactions while preserving immediate duplicate-worker
rejection. Revalidation also resolves only the current caller's active manifest
and exact method, retaining fresh Home authority checks. Installed admission,
reply and restart/reuse remain pending until the repaired Runtime passes.

The early throughput estimate is provisional. Process samples taken after the
terminal failure show idle processes and establish no preparation performance
claim. The next experiment uses terminal state and progress together.

The repair passes the reproduced short-snapshot case, the nine caller descriptor
and membership cases, existing affordance resolution, process-safe lock expiry,
concurrent reservation accounting and failure diagnostics with real Home grant
revalidation. Repository whitespace, Home entropy and both Rust format gates pass.
The independent review confirms bounded snapshot waiting and preserved worker
exclusion. The next operation rebuilds Runtime only for the same test Home.


## Existing-identity read repair

Installed Mac Runtime `2b95b382` passes the previous snapshot failure point and
prepares 812,801,875 bytes under normal UI status polling. Marketplace cancellation
then stops the bounded experiment without JavaScript errors. The operation remains
unadmitted. A sample taken while preparation advances identifies repeated identity
directory sync during every small Content read's Home authority validation.

Gateway validation now uses an existing-only identity reader. It requires the
current root, directory, lock and key, retains descriptor/permission checks and
credential decryption, and performs no durability write. Initialization and
recovery retain their existing durable sync and replacement behavior. Independent
review confirms this separation. Missing state creates nothing; changed keys,
corrupt credentials and unsafe paths fail. The identity suite passes 54 tests;
an added portable existing-reader test and all nine Home-token authority cases
also pass. Whitespace, Home entropy and both Rust format gates pass. Installed
performance and admission remain pending until the affected Runtime is rebuilt.


## Installed admission and readiness polling

Mac Runtime `ab993a99` completes the same preparation operation after the observer
reopens Home. The admitted package contains 6,169,366,387 content bytes plus its
727-byte object index. The weight SHA-256 is
`d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`.
The activation record binds this package and the reviewed native engine. Existing
Homes and preview services retain their earlier artifacts.

The installed Marketplace view receives admitted status before activation finishes,
then stops polling. Runtime later records successful activation while that view
still says the offer is unavailable. The rendered regression reproduces the stale
status. The shared helper now polls admitted content until dispatch readiness,
using the existing polling limit and visibility guards. Cancel eligibility stays
with active preparation. Both Marketplace and System pass transitions with both
activation-flag values, unchanged Use count, absent Cancel after admission, and
polling stopped at ready. Independent diff review finds no blocking issue; Home
entropy, whitespace and both Rust format gates pass. UI installation and the
actual reply/lifecycle journey remain pending.

Linux capacity preflight leaves an additional 13.4 GB needed for the same local
package import and reserved preparation space, before build margin, while keeping
the required ten-percent free-space floor. Linux model proof is pending; its
existing source checkout, public service and previews were read only.


## Model consumer catalog access

A fresh installed Marketplace view shows the admitted Qwen model ready, with
matching CID and offer. The first Assistant check then stops before dispatch:
Home Agent receives HTTP 403 for the catalog read and can show only the offer,
without its content identity. Standalone Assistant uses the same catalog route.
The original read allowlist includes Home, Marketplace, System and shells, but
omits both model consumers.

The repair adds the two existing consumers to that explicit read allowlist.
Existing token, origin, session and invocation checks retain their ownership.
The regression first reproduces the denial, then passes 30 cases across both
consumers and the three affected read routes: valid authority succeeds; missing,
expired, unrelated-app and wrong-origin authority fail. Existing Home Agent run
binding and unrelated-capsule model rejection also pass. Independent source
review finds no blocking issue. The next installation changes Runtime and the
two readiness-polling assets; the admitted package and providers are reused.


## Cold model startup prerequisite

Runtime `f72810fb` and both readiness assets are installed on the isolated Mac
Home with matching build, installed and served hashes. Installation also fills
missing Marketplace/System browser asset records. The prior manifest is recovered
exactly by removing those two records and comparing its recorded SHA-256; remaining
artifacts and the admitted package retain their identities.

Cold startup then reports an unavailable directory-hash backend and skips model
registration. IPFS is registered with a lazy backend; admitted-model startup calls
its private hash operation without first requesting readiness. The cold fixture
reproduces the missing prerequisite. The repair calls existing Runtime-owned backend
preparation after local catalog/engine/file validation and before hashing, under
the current worker guard. Integrity checks and package identity remain mandatory.

The startup module passes 23 tests, including cold success, readiness failure,
unchanged package/configuration, no new Content reads, identity, activation and
retirement. Four native/environment cases remain ignored in that source run.
Independent review finds no blocking issue. Installed Runtime `fbf1a4b0` now
advertises the same offer after cold startup without another Use or model transfer. The Runtime startup profile currently accepts Mac ARM64;
Linux needs its reviewed profile and target proof as well as disk headroom.


## Installed reply, reload and Stop

The Mac test Home runs Runtime `fbf1a4b0`, source tree `7087ce4c`, with matching
built and installed SHA-256 `b53127c3429b6f4d4a368cbd808d0f39fec70bbebf205c5a416865dc53bda54f`.
The current installation receipt binds reused providers, engine and capsule assets.
The exact admitted Qwen content selects its recorded offer, starts a native engine
and answers “Qwen is ready.” through the typed run contract. The saved conversation
acknowledgment binds that reply to the same run, offer, CID and principal. Reload
preserves the completed reply and an exact unsent test draft without implicit runs.

Explicit Stop returns `reconciling`, followed by terminal `settlement_unknown`.
The UI shows “Outcome unknown” and preserves it after reload. This passes the honest
unknown alternative in the approved J3 Step 5; it does not prove backend cancellation.
Independent review confirms the intentional local/hosted stream-closure policy and
the existing native cancellation regression. Owned shutdown separately closes all
29 processes, including the engine and its guard, the held HTTP connection and port.
The same installed Home restarts and produces a second typed reply. Its saved
acknowledgment binds the new run to the same principal, CID and offer. The original
draft is restored at workspace revision 13; both prior conversations retain their
message hashes. All six admitted files match by SHA-256, size, inode and modification
time across shutdown, restart and final observation. Their total is 6,169,367,114
bytes, including the 727-byte object metadata file. Protected identity files match.
There are zero Content mutations and zero Bitswap payload bytes received during
the measured restart/reuse interval. Independent review accepts this bounded
Mac milestone; workbook E-109 records its scope.

Private raw reports preserve the first test failures: selection checks ran before
asynchronous model refresh settled, then an expanded menu intercepted the picker.
The harness now checks stored CID/offer, waits for the exact selected row and uses
the supported Escape dismissal. The product menu still shows stale “No model” text
above a ready model and can overlap its button; these remain explicit UI repair work.
Full J3, Linux target proof, one Assistant and original URUX parity remain pending.


## Model menu and source-install metadata

Source `8f28b6e3` repairs the ready-content empty message and menu placement after
asynchronous refresh. The rendered regression first reproduces both faults, then
passes with draft/CID preservation and normal picker clicks. Independent review
accepts the small change to Sash's existing placement logic and styles.

The installed Mac check verifies one exact Qwen row, zero empty-state paragraphs,
a 10 px picker gap after opening and two refreshes, normal picker-close behavior,
and an unchanged complete workspace after reload. The source, installed and served
JavaScript SHA-256 is `6b12a07c6c6291373176bb47262a6886ed697d9f280cca5b8032355023fb906a`;
Runtime remains the reused `fbf1a4b0` build. The update preserves 756 other files.

The first launch exposed copied release-cache metadata: the new source asset
record had no archive identity, while the external component and sidecars named an
older archive. Correcting that source installation restores launch without an
archive download. Keep the managed external entry and install paths, exact source
entrypoint/browser hashes and installation receipt. Release archive identity is
assigned at the release gate. The durable stamper repair clears stale archive
fields only for selected source capsules on the current platform after parity
checks; unrelated releases and native providers remain intact. Its regression
covers stale identity, all six Runtime platform aliases, explicit empty-map
precedence, other-platform preservation, repeated stamping and failure before
mutation. Conflicting install paths fail with metadata and sidecars unchanged.
Independent source review and all four basic repository gates pass. Final installer packaging and signing remain separate.

## Corrected URUX integration status

The user's September 11 history check corrects the proposed full URUX merge.
Both the active website branch and `feat/0.7.1-integration@6972e165` already contain
`feat/0.7-uiux-candidate@8b547590`, reconstruction `7dd1780b`, and Irzhy's
`985fdffc` merge into `upstream/0.7-dev`. PR59 and PR62 also contain that chain.
The original `5e546ef4` tip has separate ancestry because the UI was adapted.
Its absence from the ancestry is insufficient evidence that its behavior is absent.
The 79-conflict preview is historical analysis; the blanket merge recommendation
is superseded. No actual URUX merge was started.

| Already integrated behavior | Source evidence |
| --- | --- |
| Shared theme/fonts, shell chrome, Control Centre, Exposé, keyboard, rails and stages | `b3aa6767`, through the UIUX candidate and reconstruction chain |
| Lock face, desktop defaults, Apps/Shelf and recovery setup | `3b483d9c`, `ec2b2f0c`, `e75cc76f`, `6df58755`, followed by current recovery repairs |
| Newer Sash dock width/reorder, dock-width Apps face, stationary icons and slot arrival | `f175b707`, `8c39c89d`, `05ff77be`, `c9a4a661`; these supersede older choreography |
| URUX lock appearance on current Home authority | Sash `b26f5ee4` |
| Capsule-owned Home Agent harness | Sash `77498557`, original history reconciled by `37c82d4f`; later model and workspace repairs retained |
| Standalone Assistant Chat/Build/Studio | `77e647a9`, `9b37e3c7`, through the UIUX/reconstruction chain |

Next, compare the existing integrated behavior with the intended user experience
and restore confirmed gaps. Richer Workbench/Studio UI and the generic Viewer rail
remain separate from current implementations. Pager/window-switcher template
nodes are absent despite retained JS/CSS. Agent Space's exclusion from Mission
Control is an explicit later choice, requiring a product disposition rather than
an assumption that code was lost. Preserve newer Sash motion and current typed
Runtime authority, recovery, Qwen selection and lifecycle behavior.

The Home Agent capsule commit explicitly removed Workbench, Studio and other
controls whose Runtime operations were unavailable. Standalone Assistant has
Chat/Build/Studio under the typed contract. Account for both implementations
before deciding how to expose one Assistant; their existence is not evidence of
complete original feature parity.

Preserve all three possible state locations: legacy
`homeBrowserState.session.agent`, protected Home Agent and protected Assistant.
Carry the raw legacy object through unrelated shell saves; history and drafts
must survive independently of presentation/context limits. Resolve concurrent
409 saves and lossless history before installing the combined Assistant. Original
URUX modules remain implementation evidence for confirmed gaps. Independent
re-review accepts this corrected route; all 70 branches and 12 worktrees remain.
