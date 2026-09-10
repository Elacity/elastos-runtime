# Release execution

A person opens the public website, understands what they can use, and opens
Home on the same server. The server runs a reviewed candidate assembled from
the active source work. Its deployment receipt identifies the source and served
artifacts. A public download version has a separate verification record.

## Record ownership

| Record | Owns |
| --- | --- |
| Approved release plan linked from TASKS.md | User outcomes, required acceptance and scope decisions |
| TASKS.md on the active work branch | Goal status, dependencies, owner and next work |
| state.md | Current source, installed and public evidence, with its date |
| Journey audit workbook | User journeys, regression expectations and findings |
| Private current checkpoint and receipts | Worktree/target coordinates, resource owners, exact commands and raw proof |
| Notion overview | Accepted milestone summaries and links to reviewed source |

The coordinator serializes status updates. Each implementer owns a bounded set
of source files and the complete repair/test cycle. Another agent reviews the
actual diff and evidence. Human acceptance retains its human author.

Use the development-loop procedure for dependent work. Keep one coordinating
mission goal and the existing journey IDs. The current checkpoint is
`development-loop-current.md` in the Git common directory. Resolve that directory
with `git rev-parse --path-format=absolute --git-common-dir` before resuming.
Read the latest user correction, then verify the named checkout and target.

## First pilot

The first work is a useful storefront and a human Home entry point. This gives
the execution procedure a real product result to verify while the remaining
release work keeps its full acceptance.

| Milestone | Observable acceptance |
| --- | --- |
| C1-site | The landing page explains the product, opens `/home/`, works at desktop and mobile widths, and supports keyboard use. Published installer, source candidate and hosted Home have distinct facts. Missing, mismatched or unsupported release data leaves an honest available action. A computed checksum is identified as a checksum. |
| C1-home | `/home` resolves to `/home/`; index and relative assets use the existing Home capsule handler and response policy. Old root bookmarks reach the new entry. Sign-out returns there. Capsule launch identity, API paths and Runtime authority remain coherent. |
| C1-proof | Source checks, actual rendered success/failure states and an independent review pass at the named revision. The process resumes at the same next action and detects stale proof, owner conflicts and repeated failures in bounded scenario tests. |
| C1-seed | An approved candidate is installed on the seed with preserved identity and user state. Site and Home artifacts match its receipt. The public root and `/home/` work; sign-in, sign-out and an ordinary app launch have target evidence. |

C1-site can be reviewed as its own source slice. Public deployment follows the
target's acceptance and approval gate. The full signed three-platform installer
and update journey remain part of the release, even when this first pilot passes.

## Verification recipes

Start with the smallest relevant failure: a metadata response, one Home route,
or one installed interaction. Then run the changed surface through its consumer.

```sh
git diff --check
node scripts/home-entropy-check.mjs
node scripts/website-truth-check.mjs
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
```

The owner adds narrow route tests and the existing Home/browser smoke checks
that cover the change. The journey workbook records expected route changes in
the same diff. Preserve existing Pass expectations and each failed run's scope.
Source checks qualify source; installed and public claims require matching
artifacts and real target behavior.

For the website, inspect desktop and mobile layout, keyboard tabs/actions,
missing metadata and inconsistent head/release data. Resolve local assets and
the exact action URLs. A visual prototype is evidence about the page only.

For Home, test the index, relative modules, manifest, old bookmark redirect,
sign-out, authenticated API/event access and ordinary app launch. Before a
target test, bind the source, built binary, installed binary and capsule tree.
Record a restart and the observed result through the public route.

## Monitoring decisions

The existing heartbeat reads the current checkpoint and changed evidence.
It observes the active coordinator and its explicit owners. Old paused tasks
keep their pause until an assignment transfers their work.
The monitor writes observations and pending corrections to its private shared
state in a writable operator folder. The coordinator reads that record at each
changed milestone and every 30 minutes, then records adoption in its checkpoint.
This keeps observation and implementation ownership separate without requiring
the monitor to send task messages or change repository metadata.

An evidence checkpoint is due every 30 minutes during long work. Report passed,
failed and pending milestones, the newest useful result and the next experiment.
Measure time to useful proof, repeated experiments, reopened acceptance and
reviewer corrections. Commit count and status edits measure activity only.

| Observation | Next action |
| --- | --- |
| Source/artifact changes after proof | Mark affected proof stale, preserve history, recheck affected consumers. |
| Same failure and unchanged experiment twice | Diagnose the repeated boundary and choose a different small experiment. Check whether a previous correction was adopted before sending it again. |
| Long build with current output | Retain its owner; report progress and its next evidence checkpoint. |
| No useful result and no live work at the checkpoint | Inspect the owner and prerequisites; record a concrete next experiment or missing resource. |
| Two writers or target owners overlap | Preserve the active owner; queue the dependent operation and continue independent work. |
| A new feature or weaker acceptance enters the diff | Restore the agreed scope or seek the user's decision with the concrete change. |
| Native task retrieval fails | Record the observation limit, read the short checkpoint and named receipts, and avoid treating missing telemetry as failure. |

The monitor reports meaningful accepted progress, material drift/failure and
required user input. It remains quiet during unchanged routine work. Local
scheduled monitoring requires the app and host to be running.
