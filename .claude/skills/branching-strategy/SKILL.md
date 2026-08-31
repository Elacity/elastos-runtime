---
name: branching-strategy
description: Use when creating a new branch or git worktree for a feature, bugfix, or experiment — before choosing a base branch or running `git checkout -b`, `git switch -c`, or `git worktree add`.
---

# Branching Strategy

## Overview

New work branches start from a user-selected `upstream/XX-dev` integration branch. The current checkout and `main` are not implicit bases. Resolve available lines from fetched refs; the user chooses the base.

`main` is the release line. `upstream/XX-dev` branches are the versioned dev/integration lines where feature and bugfix work lands.

## Workflow

1. **Refresh and discover bases.** Remote-tracking refs go stale here; fetch first:
   ```bash
   git fetch origin --prune
   git branch -a --list '*upstream/*'          # upstream/XX-dev lines
   git branch -a --list '*feat/*' --list '*fix/*'  # active working branches
   ```
   If remote state is in doubt, cross-check with `gh api repos/<org>/<repo>/branches --jq '.[].name'`.

2. **Ask the user which base to start from** (AskUserQuestion). Always ask, even when only one `upstream/XX-dev` exists. Offer as choices:
   - Each `upstream/XX-dev` branch — recommend the newest version.
   - "An existing working branch" — for work that depends on another in-flight branch.

3. **Create the branch** (name it `feat/<slug>` or `fix/<slug>`, matching existing repo conventions):
   ```bash
   git checkout -b fix/<slug> <chosen-base>
   # or isolated:
   git worktree add ../<repo>-<slug> -b feat/<slug> <chosen-base>
   ```

4. **Dependent base? Flag the merge-back immediately.** If the base is a working branch rather than `upstream/XX-dev`, tell the user in your response, and repeat it when the work is ready to merge:
   > ⚠️ This branch is based on `<parent-branch>`, not `upstream/XX-dev`. It must merge back only after (or together with) its parent, and merging into `upstream/XX-dev` may conflict with the parent's changes. Rebase onto the parent if it moves.

5. Don't push or set an upstream tracking branch until the user asks.

## Quick Reference

| Situation | Base | Extra step |
|---|---|---|
| Feature / bugfix | `upstream/XX-dev` chosen by user | — |
| Work depending on an in-flight branch | that working branch, if user confirms | Merge-back conflict warning (step 4) |
| Base unclear / user unavailable | stop and ask | Never guess |

## Common Mistakes

- **Branching from current HEAD or `main` without asking** — the checked-out branch is not evidence of the right base. List the `upstream/*-dev` lines and ask.
- **"Only one dev branch exists, so no need to ask"** — still ask; the user may want a dependent working branch as base, or a new dev line may be pending.
- **Silent dependent branch** — creating a branch off another feature branch without stating the merge-back risk leaves the user to discover conflicts at PR time.
- **Trusting stale remote refs** — always `git fetch origin --prune` before listing candidate bases.
