---
name: branching-strategy
description: Use when creating a new branch or git worktree for a feature, bugfix, or experiment — before choosing a base branch or running `git checkout -b`, `git switch -c`, or `git worktree add`.
---

# Branching Strategy

`develop` is the integration line. Task branches start from `develop` and merge back into it. `main` is the release line and changes only through the weekly release PR. The owning issue may name another base.

## Workflow

1. Fetch first, because remote-tracking refs go stale:
   ```bash
   git fetch origin --prune
   ```
2. Use the base the owning issue names; otherwise use `origin/develop`.
3. Create the branch as `feat/<slug>` or `fix/<slug>`:
   ```bash
   git worktree add ../<repo>-<slug> -b feat/<slug> origin/develop
   ```
4. If the base is another in-flight branch, say so in the PR: the branch merges after its parent and needs a rebase if the parent moves.
5. Pushes follow the Push Approval Levels in `AGENTS.md`.

## Common Mistakes

- Branching from the current checkout or from `main`. The checked-out branch is not evidence of the right base.
- Trusting stale remote refs. Fetch before branching.
- A silent dependent branch. State the parent in the PR so the merge order is clear.
