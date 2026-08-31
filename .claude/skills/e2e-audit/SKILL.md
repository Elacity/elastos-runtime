---
name: e2e-audit
description: Use when preparing, reviewing, or merging any PR, planning or cutting a release, changing product or GUI behavior, or assessing stability, correctness, or regression risk in this repo — before declaring work done or release-ready.
---

# Journey Audit Register

## Overview

[docs/audits/ElastOS-Home-Journey-Audit.xlsx](../../../docs/audits/ElastOS-Home-Journey-Audit.xlsx)
is the canonical register of user journeys, static controls, and findings. It is
the reference for stability and correctness for every PR and release. Treat it
as a contract: product-behavior changes are measured against it, and the
register is updated in the same PR that changes what it describes.

## Reading the workbook

Seven sheets:

| Sheet | Contents |
|---|---|
| Coverage | Audit metrics (195 journeys mapped, 249 static controls, 22 installed UI capsules) |
| Journey Matrix | One row per journey: ID (`AUTH-01`), Area, Journey, Live status, Expected behavior, Actual/exact scope, Safety, Source state, Controls/branches, Steps, Prerequisites, Findings, evidence columns |
| Control Index | 249 static controls with owners |
| Findings | `F-xx` rows: Priority, Finding, Classification, Disposition, Evidence basis, User impact, Root cause, Smallest next change, Proof to close, Journey IDs, Source refs |
| Prioritized Plan | Repair ordering for open findings |
| Read Me + Evidence | Evidence register (private-evidence labels, not repo links) |
| Principles Review | Finding-by-finding principles check |

`openpyxl` is not installed on dev machines; use the bundled stdlib dumper:

```bash
python3 .claude/skills/e2e-audit/read-workbook.py                   # list sheets
python3 .claude/skills/e2e-audit/read-workbook.py "Journey Matrix"  # TSV dump
python3 .claude/skills/e2e-audit/read-workbook.py Findings --grep AUTH-01
```

## The invariant

1. Before changing behavior in an area, read that area's Journey Matrix rows
   and every Finding whose Journey IDs reference them.
2. A journey recorded as Pass stays Pass. A change that intentionally alters a
   journey's expected behavior updates the journey row in the same PR and names
   the change in the PR description.
3. A finding closes only with its "Proof to close" evidence. Dispositions are
   updated in place, never deleted.
4. New product surface ships with new journey rows (ID, Area, Expected
   behavior, Steps, Prerequisites) in the same PR.
5. Verdicts are evidence-bound. Live status and Live evidence come only from
   installed, live checks; source reading updates Source state only. The
   snapshot pins installed evidence to an exact source commit — later source
   changes need their own installed checks before any live claim.
6. Release gate: no open P1 finding against a touched area, and a refreshed
   audit snapshot for the exact release revision. The register drives
   pre-release testing.

## Common mistakes

- Claiming a journey passes because code compiles or unit tests pass — the
  register separates Source state from Live evidence for exactly this reason.
- Changing an area without searching the workbook for its journey IDs.
- Reading the evidence labels as repo paths — evidence files stay outside the
  repository (see [docs/audits/README.md](../../../docs/audits/README.md)).
