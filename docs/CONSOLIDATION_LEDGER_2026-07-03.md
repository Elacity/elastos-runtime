# Branch Consolidation Ledger — 2026-07-03

**Goal:** collapse every outstanding branch into one line (`flint-0.5`) with
**zero value lost**, verified by CONTENT (symbol/file audits), never by branch
name or commit-SHA reachability (re-authored work has different SHAs but the
same content).

**Method:** the consolidation was assembled on the session workbench branch
`claude/git-proxy-auth-roadmap-c214hu` (this session can only push there), gated
at every slice (build → test → clippy), and is delivered as **one pull request
into `flint-0.5`** — the human clicks Merge once and `flint-0.5` holds everything.

Workbench tip vs the old `flint-0.5` tip (`9af4177`): **64 files, +12,856 / −239.**

---

## What the workbench now adds on top of `flint-0.5`

| Commit | What |
|---|---|
| `238e1e5` `939e44a` `6798942` `04052ef` `40086d3` | the team's **`fix/mpeg-dash-compliance`** delta (MPEG-DASH/CENC compliance, DKMS quorum reliability ELACITY-2282/2283, code-review hardening) absorbed via `--no-ff` merge |
| `b419da2` | **recovered** the Elacity Bible (`docs/narrative/ELACITY_BIBLE.md`) — the only unique commit on the deleted `claude/elacity-narrative-strategy-pmsr9j`; nearly lost |
| `84ae217` | **restored** WASM epoch operator-termination (dropped in the mpeg-dash squash) + made it race-free (red-team fix) |
| `451faab` | **restored** media transcode-progress reporting (dropped in the mpeg-dash squash) |
| `8d14230` `26abf3c` `2e8c3db` `a2dd4af` `b12528e` | **marketplace** transplant (slices 1–5): chain resolver, buy/trade authorities, `/api/market/*` + content-index, library Acquire, storefront capsule + Home wiring |
| `dd7ec4b` | red-team hardening + registered findings (MKT-1..4) + `acquire` conformance |
| `ab10be0` | preserved the `w2-consent-source` unique commit as a patch + decision note |

---

## Per-branch disposition (all 13 live branches + the one already deleted)

| Branch | Disposition | Evidence | Safe to delete? |
|---|---|---|---|
| **`flint-0.5`** | **TARGET** — the workbench PR merges here | — | **NO — keep. This is the one branch.** |
| `claude/git-proxy-auth-roadmap-c214hu` | workbench (session-scoped) | holds the consolidation; the PR source | after the PR merges |
| `fix/mpeg-dash-compliance` | **ABSORBED** | its delta is merged into the workbench (0 unique commits vs workbench) | after PR merges (PR #11 becomes redundant with the merged history) |
| `feat/marketplace-runtime` | **TRANSPLANTED** (slices 1–5) | market API/capsule/authorities/Acquire all re-landed + gated; S6 dkms superseded by flint-0.5's better retry-once | after PR merges + a spot-check |
| `feat/ddrm-hardening-and-creator-parity` | **CARRIED** | content re-authored into `fix/mpeg-dash-compliance` (now absorbed) + the 2 dropped features restored (`84ae217`,`451faab`) | after PR merges |
| `claude/keep-consent-architecture-0fz0ll` | **SUPERSEDED** | audit: every symbol (DidNotAct, read_bounded_line, EgressFirewall…) present in flint-0.5 with tests; **0 unique code files** | yes (after PR merges) |
| `feat/capsule-inspector` (PR #6) | **SUPERSEDED** | audit: approval core, inspect/intent, ratchet registry, inspector capsule all in flint-0.5; **0 unique code files** | close PR #6 as delivered; then delete |
| `w2-consent-source` | **PARTIALLY SUPERSEDED — 1 preserved** | 2/3 commits superseded; commit `3694975` (gateway 202-consent seam) banked as `docs/patches/w2-gateway-consent-request-3694975.patch` + decision note (collides with flint-0.5's pinned flat-403; needs an architecture decision) | yes (the patch preserves the unique work) |
| `claude/branch-deep-audit-yiez86` | **SUPERSEDED** | 0 unique code files vs workbench (audit/docs branch) | yes |
| `review/0.5.0` | **SUPERSEDED** | 0 unique commits vs workbench | yes |
| `flint` | **SUPERSEDED** | fully contained in flint-0.5 (0 unique commits) | yes |
| `claude/elacity-narrative-strategy-pmsr9j` | **already deleted — value RECOVERED** | its only unique commit (Elacity Bible) is restored as `b419da2` | already gone; nothing lost |
| `upstream/0.6-dev` | **TEAM-OWNED — do not touch** | the team's audit-staging base; PR #9 (`flint-0.5`→`0.6-dev`) is theirs to merge | keep |
| `main` | **LIVE — never touch** | production | keep |

---

## Registered follow-ups (build-visible in `KNOWN_GAPS.md`)

- **MKT-1 (HIGH, fix before the marketplace ships):** the KID→tokenId resolver
  can mis-bind to a hostile co-channel mint (pre-existing in the marketplace
  source, transplanted faithfully). On-chain-reachable; the buyer can pay for an
  attacker's token. Not client-API-reachable.
- **MKT-2/3/4 (hardening):** unbounded resolve RPC fan-out; media `progress_path`
  unconfined; ffmpeg-progress stdout-read deadlock window. All pre-existing.
- **w2 consent seam:** decide runtime-intent-envelope (current) vs gateway
  202-consent (banked patch) — one consent story.

## The human's two actions

1. **Merge the PR** (workbench → `flint-0.5`). `flint-0.5` then holds everything.
2. **Delete** the branches marked "yes" above (GitHub UI — the session proxy
   blocks ref deletion). Keep `flint-0.5`, `upstream/0.6-dev`, `main`.
