---
name: branching-strategy
description: Use when creating a new branch or git worktree for a feature, bugfix, or experiment — before choosing a base branch or running `git checkout -b`, `git switch -c`, or `git worktree add`.
---

# Branching Strategy

Follow **Creating Work Branches** and **Branch Roles** in the root `AGENTS.md`. They are the only source for bases, branch names and merge targets.

The usual case after `git fetch origin --prune`:

```bash
git worktree add ../<repo>-<slug> -b feat/<slug> origin/develop
```
