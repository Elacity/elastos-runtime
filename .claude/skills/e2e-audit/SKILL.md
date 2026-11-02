---
name: e2e-audit
description: On-demand only — invoke with /e2e-audit. Reads the historical Home journey-audit workbook as test input for PR review and regression assessment in this repo.
disable-model-invocation: true
---

# Journey Audit Workbook

## Overview

[docs/audits/ElastOS-Home-Journey-Audit.xlsx](../../../docs/audits/ElastOS-Home-Journey-Audit.xlsx)
is a historical snapshot of user journeys, static controls, and findings. It is
test input, not a current record. Current acceptance lives in GitHub: each
owning issue keeps its Acceptance record (see
[AGENTS.md](../../../AGENTS.md#acceptance-record)), and #93 owns the release
gates. The workbook is not updated.

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

## Using it

1. Before changing behavior in an area, read that area's Journey Matrix rows
   and every Finding whose Journey IDs reference them.
2. Transfer the relevant journeys and open findings to the owning GitHub issue
   as acceptance checks. Record results there, not in the workbook.
3. Keep journeys that passed working. A change that intentionally alters one
   names the change in the PR description and the owning issue.
4. Live status in the workbook is pinned to its snapshot source commit. Any
   current live claim needs installed evidence from the exact candidate.

## Common mistakes

- Treating the workbook as current status — its verdicts date from the
  snapshot; the owning issue holds current status.
- Claiming a journey passes because code compiles or unit tests pass — the
  workbook separates Source state from Live evidence for exactly this reason.
- Reading the evidence labels as repo paths — evidence files stay outside the
  repository (see [docs/audits/README.md](../../../docs/audits/README.md)).
