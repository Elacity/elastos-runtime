# ElastOS 0.7.1: contributor branches and PRs

**Checked 11 September 2026.** Companion to the [5 to 11 September team report](2026-09-11-team-sync.md). GitHub API metadata, local ancestry and selected patch/tree comparisons were checked against delivery candidate `06bf4e0f`, tree `4483f403a85d1ac27b4568f7b8cb4cf1a8a4e546`.

At publication, the reviewed candidate was 79 commits ahead and 0 behind the earlier public website checkpoint at `00003b6f`; both histories are now protected by integration. Local adoption and GitHub PR state are shown separately. “Ancestor” means the original commit is in candidate history. “Adapted” means work was transferred or revised through a different commit. Neither term certifies complete installed behavior.

The existing `feat/0.7.1-integration` branch was fast-forwarded to this candidate and published in [draft PR64](https://github.com/Elacity/elastos-runtime/pull/64) into `upstream/0.7.1-dev`. Remote verification protected both website heads before the redundant local/remote website refs and checkout were removed. This companion accompanies the single documentation-only closeout; PR64 records its final commit and immutable report links. PR58's merge remains in history; PR60's original head is included in the published candidate; PR59 and PR62 remain pending. All contributor PR heads, bases and states were preserved.

## September inventory

The existing 13-PR inventory covers PRs created or updated since **1 September**, so it includes work older than this report's 5 to 11 September delivery window. Sash is `SashaMIT`; Irzhy is `irzhywau`. PR authorship and individual code authorship can differ, particularly in integration PRs.

| Contributor / PR | Head branch and commit | GitHub state | Interpretation at `06bf4e0f` |
| --- | --- | --- | --- |
| Sash [19](https://github.com/Elacity/elastos-runtime/pull/19) | `chore/ci-source-home-matrix` · `1d6b49e3` | Closed, unmerged | Separate original ancestry. Current CI retains three-platform source installation, manual ref selection and installed-provider verification. macOS Clippy/tests moved to Linux jobs, and named feature push triggers became PR/manual/main/tag triggers. These differences need CI policy review. |
| Sash [23](https://github.com/Elacity/elastos-runtime/pull/23) | `feat/home-urux-on-freeze` · `5e546ef4` | Closed, unmerged | Adapted UI entered through `8b547590`, Irzhy's `7dd1780b` and merge `985fdffc`. Later Sash dock/lock changes are also ancestors. Compare remaining behaviors; the old tip is not the next merge target. |
| Sash [26](https://github.com/Elacity/elastos-runtime/pull/26) | `feat/gba-nonogram-advance` · `7c622705` | Open | Adopted through `1fd30b38`, with separate original ancestry. ROM, MIT licence and SVG blobs match exactly. Current icon metadata/assets and attribution reflect later changes. |
| Irzhy [38](https://github.com/Elacity/elastos-runtime/pull/38) | `upstream/0.7-dev` · `e481b153` | Merged | Original release-source head is an ancestor. Historical foundation with September activity, rather than new implementation this week. |
| Irzhy [51](https://github.com/Elacity/elastos-runtime/pull/51) | `upstream/0.7.1-dev` · `6c61c990` | Open | The head is the public dev baseline and an ancestor. This PR releases dev to main after candidate acceptance. |
| Irzhy [52](https://github.com/Elacity/elastos-runtime/pull/52) | `feat/protected-content-installed-provisioning` · `4d688cc5` | Merged | Original head is an ancestor. Custody/chain provisioning and startup reconciliation are included. |
| Sash [54](https://github.com/Elacity/elastos-runtime/pull/54) | `feat/home-first-run-seed-0.7.1` · `2a49ea57` | Merged | Original head is an ancestor. Includes first-run seed, lock face, Documents/Library/chrome fixes, dock behavior and capsule-owned service icons. |
| Sash [55](https://github.com/Elacity/elastos-runtime/pull/55) | `feat/home-shelf-assistant-face-0.7.1` · `923193bb` | Closed, unmerged | Adapted content arrived through integration. Local merge `37c82d4f` records original ancestry. Three patches have matching stable patch IDs; the original and adopted harness commits have the same capsule tree. |
| Irzhy [57](https://github.com/Elacity/elastos-runtime/pull/57) | `feat/protected-content-installed-e2e-proof` · `617796a9` | Closed, unmerged | Replaced by PR60 after retargeting/rebase. Both now identify the same head. Count this implementation once. |
| Irzhy [58](https://github.com/Elacity/elastos-runtime/pull/58) | `feat/0.7.1-integration` · `5ba1faa0` | Merged | Published head is an ancestor. Local merge `b318bfda` incorporates the later donor at `6972e165` too. |
| Irzhy [59](https://github.com/Elacity/elastos-runtime/pull/59) | `feat/protected-content-atomic-cutover` · `25ab205e` | Open | Head is outside candidate ancestry. Follows PR60. Runtime authority cutover and API removal need review against the current callers before integration. |
| Irzhy [60](https://github.com/Elacity/elastos-runtime/pull/60) | `feat/protected-content-installed-e2e-proof` · `617796a9` | Open | Full original history is merged locally by `dd21d8bd`. Includes provider hosting, peer seeding, availability/media repairs and installed-proof tooling. Its GitHub PR remains open. |
| Irzhy [62](https://github.com/Elacity/elastos-runtime/pull/62) | `feat/protected-content-0.7.1-followup` · `06179578` | Open, draft | Head is outside candidate ancestry and follows PR59. Its four commits contain the plan, wiring/cleanup, Creator and non-media protection/read support. Reader capsule, audio and crypto-review completion remain pending. |

## Earlier sources needed to interpret 0.7.1

| Contributor / PR | Head branch and commit | GitHub state | Interpretation |
| --- | --- | --- | --- |
| Irzhy [39](https://github.com/Elacity/elastos-runtime/pull/39) | `feat/protected-content-uiux-reconstruction` · `7dd1780b` | Merged | Ancestor through `985fdffc`. Carries adapted Sash UIUX plus Archive Manager reconstruction, typed Assistant workspace, Home menu protocol and CI work. |
| Irzhy [43](https://github.com/Elacity/elastos-runtime/pull/43) | `fix/protected-content-mint-intent-adoption` · `58ebfb23` | Merged | Ancestor. Preserves completed mint work after a lost completion mark. |
| Irzhy [15](https://github.com/Elacity/elastos-runtime/pull/15) | `feat/dkms-esp-port` · `27d85c6f` | Open | Separate ancestry. The project retains an extraction ledger for protected-content behavior. Remaining parity and audit-migration decisions need explicit disposition; a wholesale merge is not implied. |
| Sash [17](https://github.com/Elacity/elastos-runtime/pull/17) | `feat/model-provider-rebase` · `9f423796` | Open | Separate ancestry. Credit the earlier model-provider work. The candidate uses a later typed-provider implementation; complete feature equivalence requires a scoped comparison. |
| Irzhy [25](https://github.com/Elacity/elastos-runtime/pull/25) | `feat/elastos-logger` · `66c8bba4` | Open | Separate ancestry. The current candidate has no `elastos-logger` crate. This independent logging proposal needs an explicit scope/disposition decision. |

Earlier protected-content extraction work is represented in the base source and PR15 ledger. This review checks the active intake interpretation and the relevant older open tracks; it does not claim feature-by-feature equivalence across every historical PR.

## Integration details and remaining decisions

**Sash UI preservation.** The original shelf/composer/Agent Space series was authored on 3 September. This week recorded its original ancestry and retained its UI in the unified Assistant. Patch comparisons and capsule-tree identity support that preservation: original `239c6cb7` and adopted `77498557` share tree `c03166c4489f6773792c38c337f7ccb5e714f6ce`. The URUX adaptation chain and newer dock/lock commits are also present. Review remaining Viewer rail, pager/switcher, Mission Control, launcher motion and workflow differences against the current Runtime contracts before selecting further UI work.

**Irzhy installed evidence.** The 5 to 7 September protected-content test used an installed client, three custody containers and an Anvil/Base fork. Its final receipt records passing mint, availability, buy, 2-of-3 release, ordered reads, close, denial/tamper/replay, restart and cleanup phases. The proof used headless principals; final Brave interaction, independent operator/hardware domains and real Base remain separate. Its receipts bind the binaries actually tested, which predate later lint/test edits. Commit `617796a9` retains a 5 September author date, an 8 September committer date, and entered the delivery branch on 10 September.

**PR62 scope.** The diff from `25ab205e` to `06179578` contains no media-provider changes. Media code present at the PR62 tip comes from its inherited stack. The delivery branch's media repair therefore does not establish adoption of PR62 follow-up work. Creator source exists at its current head; a Reader capsule does not. Required video-journey repairs remain required under D2, while optional additions enter only with source and installed evidence before freeze.

**Models UI ownership.** Dedicated Models screens came from integration commit `b0c4c2f5`. The bounded source review found no separate Sash/Irzhy Models design to restore. The latest repair reuses Sash's general Marketplace category/detail layout and shared theme. This separates credit for the design foundation from responsibility for the later model controls.

Before closing contributor intake, the team needs dispositions for the older open tracks, the remaining UI differences and PR59/PR62 readiness. A clean merge or a closed PR cannot make those decisions on its own.

## Check method and limits

GitHub API supplied PR states, heads, branches and descriptions. Local Git supplied ancestry, commit dates, stable patch IDs, selected blob/tree identities and the PR59-to-PR62 diff. The preservation and convergence audits supplied installed-evidence scope. This review ran no builds or installed tests and changed no contributor branches, PRs or acceptance checkboxes.
