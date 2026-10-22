# Test review register

This artifact preserves the test review for [issue #194](https://github.com/Elacity/elastos-runtime/issues/194). The issue owns current status, acceptance and decisions. The user approved a versioned register because the individual records exceed an issue body.

**This is an incomplete evidence snapshot.** The tracked-file census is complete for the pinned source. Direct body review, helper and production review, runtime discovery, and installed acceptance have separate limits. A proposed `retain` decision on a provisional row means preserve it while review continues.

The [manifest](manifest.json) identifies the source commit, Git tree, comparison base, counts and artifact hashes. Each record also binds its source path, line range, complete file hash and extracted-body hash. Review uses pinned Git objects even when a working checkout moves. The artifact branch contains the register; it does not publish the separately owned release candidate.

## Read the records

Run these commands from the repository root. The reader uses the Python standard library.

```sh
python3 docs/testing/test-review-register/register.py summary
python3 docs/testing/test-review-register/register.py find --path browser-clipboard
python3 docs/testing/test-review-register/register.py find --finding JS-F002 --json
python3 docs/testing/test-review-register/register.py find --provisional --path elastos-server
python3 docs/testing/test-review-register/register.py find --followup --path browser-input
python3 docs/testing/test-review-register/register.py verify
```

To verify every source file against the pinned Git objects, add `--git-repo /path/to/repository` to `verify`. The repository must contain the main candidate and any separately pinned follow-up commits. This check reads those commits, so the current checkout may differ.

The manifest's `source_followups` lists reviews of later candidates. Use `find --followup` to read those records. They preserve their own source hashes, counts and execution receipts and stay outside the main census totals.

| Artifact | Contents |
|---|---|
| [Rust server](rust-server.jsonl) | Server test declarations and documentation fences |
| [Other Rust](rust-other.jsonl) | Workspace, provider, tool and example declarations |
| [JavaScript](javascript.jsonl) | Expanded Node registrations, helpers and standalone checks |
| [Python](python.jsonl) | Named definitions, standalone scenarios and assertion helpers |
| [Shell](shell.jsonl) | Suite wrappers, target probes and acceptance entrypoints |
| [Embedded checks](embedded.jsonl) | Acceptance operations and workflow-embedded checks |
| [Other languages](other.jsonl) | C design proofs and compiled test fixtures |
| [Files](files.jsonl) | Every tracked input, its classification, hash and review-row count |
| [Findings](findings.json) | Grouped observations, affected IDs and specific actions |
| [Execution](execution.json) | Distinct runs, failures, controlled probes and evidence hashes |
| [Profiles](profiles.json) | Shared CI routes, isolation evidence and reviewer context |
| [Identity aliases](identity-aliases.json) | Earlier function groups mapped to current scenario identities |
| [Method review](method-review.json) | Inventory and provenance checks, with their original scope |

The JSONL files use one stable identity per line. Shared profiles remove repeated metadata without changing its content; `find --json` expands them. A finding can apply to several records. A helper finding can require caller review even when the callers have strong behavior assertions. Detailed sensitive evidence stays in private storage; public entries state the acceptance scope and owner.

## Review standard

Each assessment asks what the user or owning component needs to observe, then compares that contract with the actual test:

- Does the fixture reach the intended behavior, using the real operation or a clearly scoped substitute?
- Could an unrelated error, another invalid input, a stale constant, or the same implementation used twice satisfy the oracle?
- Do positive controls establish a valid starting point, and do refusal controls prove the intended boundary and absence of effects?
- Does the fixture own its state, environment, processes, sockets, files and cleanup, with useful deadlines and diagnostics?
- Does the runner select the case, preserve failures and skips, and bind installed or target claims to the exact artifact?

ElastOS alignment means that tests prove the owning principal, session, app and Runtime boundary where relevant. A response is different from a durable effect, a received offer is different from played media, and a completed export is different from usable recovery on a separate target. Human and agent entry paths need equivalent policy decisions.

Serialization, protocol-vector and source-policy tests retain their purpose when they protect a real contract. A test requires a specific reason and replacement evidence before removal. A passing suite alone does not establish meaningful assertions or complete user journeys.

## Interpret counts and results

Rust source declarations precede compiler expansion, target selection and feature conditions. Node table registrations can share one body. Python definitions can contain many `subTest` iterations. Shell wrappers can invoke whole suites. Helpers and standalone acceptance operations are separate kinds. Adding all record counts does not produce an executed-test total.

`body_read` records an attributed complete body read. The method, limitations and reviewer context describe the depth of helper and production review. Rule-assisted extraction remains provisional. File classifications identify the inventory method; they do not claim that every production file was reviewed line by line.

The repeated central Node run had two readiness failures and nine expected cases missing from its event accounting. Those nine passed in a separate focused run. Other focused runs found stale fixtures and missing input prerequisites. The earlier passing suite remains separate evidence. [Execution records](execution.json) preserve these distinctions; a diagnostic rerun does not erase the original result.

The artifact branch also had different completion counts in two runs of the same candidate. Its focused operator-client suite passed separately. The original missing completion records remain an open runner-accounting question; an aggregate pass alone does not resolve them.

Source routes into CI establish selection intent. Compiler discovery, platform execution, installed recovery, Browser media and cross-device journeys retain their own gates. Continue from stable IDs and body hashes; changed bodies or helper contracts require reassessment. The live acceptance record remains [issue #194](https://github.com/Elacity/elastos-runtime/issues/194).
