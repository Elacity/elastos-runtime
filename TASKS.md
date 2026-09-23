# Tasks

Open work only. Completed work belongs in
[elastos/CHANGELOG.md](elastos/CHANGELOG.md). Verified current truth belongs in
[state.md](state.md).

Operating principle: one canonical path per operation and clear failure when a path is not yet ready.

Guiding-star constraints live in [PRINCIPLES.md](PRINCIPLES.md).

Do not add new product surface area until the `Now` section is materially tighter.

## Browser maturity workstream

The Browser work requested on 2026-09-07 has one acceptance contract:
[Browser maturity goals](docs/BROWSER_ACCEPTANCE.md). The delivery queue below
applies within this workstream; existing release work retains its own scope.
People and agents use the same Browser authority and lifecycle. Runtime owns
device compatibility and independent local or remote placement of Engine and
Exit. These checkboxes are the canonical status; the linked document gives
instructions, dependencies, and measurable acceptance criteria.

Initial analysis source:
[`8ac18bec65ca650615be879f7ab3f66799d9fc53`](https://github.com/Elacity/elastos-runtime/tree/8ac18bec65ca650615be879f7ab3f66799d9fc53).
Revalidate those findings against the implementation base before each repair.

### Historical delivery queue and 24-hour checkpoint plan

This dated queue preserves the original scope and allocations. The Now section
owns current execution and resource assignments; these historical Active labels
do not grant a present lease.

Execution revision: 2026-09-08. The user requests the full Browser outcome by
2026-09-09, approximately 13:34 UTC, 24 hours after the deadline instruction.
B01-B16 remain the acceptance baseline. Their broad checkboxes track full
qualification; the delivery slices below drive current work. B01 stays open for
its support matrix while its accepted contract unblocks dependent work.

The task Mac passes the requested Home-to-close journey, including decoded
audio, reload and separately authorized native operator actions. Runs 92/93
add independent acceptance on the current installed artifact set, including
bounded actual Camofox and Playwright operation. Current evidence and artifact
identity are in [state.md](state.md#browser-contract-and-device-qualification).
Run 96 also completes ordinary browsing through the Linux consumer/Exit and
Mac Engine, then confirms all 13 close effects. Its viewer reload fails the
five-second recovery gate. This advances the remote contract milestone while
the integrated remote journey remains failed.
The initrd-only RNG activation removes an observed five-second bootstrap delay;
current launcher samples range from about 9 to 11 seconds. A decoded-audio
repeat fails with long silence, while a later controlled interruption passes
audio/video/input recovery. That audio failure remains unexplained. Fresh ordinary installation,
remote placements, complete operator use and release qualification need proof.
Sash's reported failure on the published source remains unverified.

| Delivery slice / acceptance mapping | Status and owner | Observable exit requirement |
| --- | --- | --- |
| B02.install: install and use Browser on a Mac; B01/B02/B03/B05/B06/B07/B11/B15 | Active; coordinator owns the Mac, installed artifacts and integration | An independently provisioned installation acquires one compatible artifact set and opens Browser through Home. Navigation, decoded video, audio, typing, scrolling, reload, interruption and close pass. Startup/input meet measured budgets, failures name the responsible stage, and a stalled launch leaves another admitted session responsive. |
| B04.operator: a human and an authorized agent operate the same page; B04/B12/B13/B14 | Active; delegated owner implements operator approval and actions | Actual Engine-page inspection, actions and waits work through Runtime authority. Human/agent handoff retains page, profile and service identity. Native, Playwright and declared Camofox/Camoufox paths pass the shared workflow; stale references and revoked writers are rejected. Profile, files, approvals and accessibility keep their own acceptance cases. |
| B10.placement: Engine and Exit move independently; B08/B09/B10/B14 | Active on the admitted Mac Engine and Linux consumer/Exit; coordinator owns target execution | The same capsule completes the same journey in A/A/A, A/B/A, A/A/B, A/B/B and A/B/C. Product service controls select approved peers; destination/DNS evidence identifies Exit. Physical LAN, supported WAN and relay-required cases prove media, recovery, revocation and cleanup. |
| B16.qualification: sustained daily use on each claimed role; B01-B16 | Planned; coordinator assembles candidate, independent reviewer checks evidence, humans perform UX acceptance | Exact candidate artifacts pass the original device, media, recovery, concurrency, profile, wallet, authority, installation/update and operator gates. Include 100 lifecycle cycles, 100 cold and 100 warm launches, 30-minute A/V interaction, eight-hour mixed use, manual UX and the objective audit. A second maintainer repeats installation and use. |

These delivery slices run through accepted contract outputs, rather than waiting
for each preceding Bxx checkbox to close. Every acceptance ID retains its full
instructions in `BROWSER_ACCEPTANCE.md`; grouping changes delivery order only.
In particular, B12 profiles, B13 daily workflows/accessibility/Wallet, B14 leases
and authority, and B15 update/repair need implementation and behavioral evidence
alongside the local, operator and remote journeys.

### Resume queue after the requested pause

Work stopped at the user's request on 2026-09-09 for a usage-reset handover.
The local branch and frozen source edits are preserved. The final private
handover identifies exact artifact receipts, process ownership, patches and
commands. A fresh session first reconciles those receipts with actual state.
Source acceptance, installed journey acceptance and full qualification remain
separate results. B01-B16 acceptance criteria and checkboxes below are unchanged.

1. **B06/B08 remote reload:** review the frozen viewer scheduling and observation
   patches, then install only the accepted UI change and repeat the same A/B/A
   journey. Keep the current Runtime/image/helper set for attribution. The next
   evidence must show timely display attachment, fresh decoded frames, retained
   page/profile/services, input and all 13 close effects. Keep run 96 failed.
2. **B02/B03/B11 Home and responsiveness:** install the reviewed `de0a299e`
   Runtime candidate and repeat Home timing on the same principal, then the
   canonical Browser journey. Measure an optimized candidate before release
   performance claims. Diagnose idle CPU and intermittent audio with matched
   producer/receiver evidence. Preserve normal fresh sign-in qualification.
3. **B12/B13 state and daily operations:** install matched host/guest profile
   protection `52238f2f`; run a new write/close/read pair for cookies, local
   storage and committed IndexedDB. Preserve failed write 94 and stopped read 95.
   Prove the reviewed 64 KiB Library upload and changed-byte rejection. Implement
   Linux profile attachment, protected checkpoints/approved transfer, crash
   recovery and ephemeral deletion. Downloads, large-file progress/cancel,
   Wallet consent/return semantics and human accessibility remain open.
4. **B04/B07/B08/B09/B10/B14 independent source and target work:** extend the
   bounded installed operator surface to the complete shared workflow; qualify
   Playwright, Camofox and declared Camoufox paths. Prove two-session progress
   under a stalled operation. Complete A/A/A, A/B/A, A/A/B, A/B/B and A/B/C with
   real approved peers, controlled destination/DNS evidence, recovery and
   revocation. Expired-service selection, pre-allocation revocation and leases
   retain their UX and authority checks. Use one owner for the shared Mac.
5. **B02/B15 installation and update:** execute the frozen staged-executable
   regression red/green tests before source acceptance. Bind the actual image
   and matching helpers to the distributable component manifest and ordinary
   installer. Prove missing/corrupt/incompatible/interrupted acquisition,
   component health settlement, profile migration/rollback and upstream Engine
   security maintenance. Sash and a second maintainer need the normal install
   journey. Publication and public deployment require their own authorization.
6. **B01/B05/B06/B11/B16 qualification:** certify each claimed role and device
   on one frozen compatible candidate. Run launch/cycle distributions, continuous
   media, mixed use, manual UX and the objective audit. The runner still needs
   valid warm conditioning, input-to-visible latency and synchronized A/V offset
   evidence. A qualifying 100-cold campaign may also supply the 100 lifecycle
   facts when every cleanup requirement is retained. Idle periods in mixed use
   do not count toward uninterrupted 30-minute A/V.
   Reconcile the objective audit's older provider-documentation predicates with
   the current Runtime contract while preserving real media and manual UX gates.

The planned soak start was missed and none of the required full campaigns has
started. The original deadline remains recorded below as history; the next
session must establish a feasible execution schedule from remaining work.
The user has not waived any acceptance requirement.

### Time and resource control

| Checkpoint from deadline instruction | Required evidence or decision |
| --- | --- |
| First 2 hours | Attempt a minimal fresh-install, operator and placement journey wherever the required target is available. For each of the five placements and each target role, record the first failing stage or the missing resource and owner. Check audio early. Establish target access and human-review availability before assigning qualification time. |
| By hour 8 | Review measured local usability and integrated operator/remote progress. Report any unimplemented contract or unavailable device that threatens the deadline. Redirect source work to those gaps; small local performance gains do not justify leaving remote/operator behavior untested. |
| By hour 12 | Produce the combined candidate and start the required eight-hour workloads on each independently available target. Complete shorter disruptive, lifecycle and launch-distribution tests before reserving a shared target for its soak. A target that cannot start now puts its full qualification past the planned review window. |
| Hours 12-20 | Run sustained-use qualification on fixed candidate artifacts. Independent review and tests on other owned targets can continue. A material repair requires the affected evidence to be repeated on the repaired candidate. |
| Hours 20-24 | Finish human and second-maintainer checks, review receipts, run the objective audit and prepare a local reviewable result with an exact pass/fail/pending matrix. Required publication and public deployment remain separate explicit actions. |

This is an execution budget, not a prediction that all requirements will pass.
Physical target access, complete operator adapters and remote service behavior
are current schedule risks. Missing evidence retains its original acceptance
requirement. Candidate-only platforms remain explicit in the support matrix;
the deadline does not change a platform's support verdict.

One coordinator owns the shared Mac Runtime, viewer, VM, builder, ports and
fixture. Three source agents receive separate file scopes and can
cross-review completed slices. Long jobs on that Mac are scheduled, because
concurrent mutations would invalidate evidence. Reuse the verified image and
matching artifacts; inspect standard dependency correctness before adding
compatibility patches or changing engines.

Each investigation names a hypothesis, the smallest discriminating experiment
and the journey it restores. At each 30-minute evidence checkpoint, report
passed, failed and pending milestones, what the user can do, the current failing
stage and the next experiment. If a checkpoint produces no new evidence, change
the experiment or the work order. Preserve useful receipts, run checks for the
touched boundary and return each repair to its installed journey. Keep source
verification separate from product acceptance and required human review.

- [ ] [B01: One Browser contract and an explicit support matrix](docs/BROWSER_ACCEPTANCE.md#b01).
- [ ] [B02: A fresh installation opens a usable Browser](docs/BROWSER_ACCEPTANCE.md#b02).
- [ ] [B03: Every failure has an actionable diagnosis](docs/BROWSER_ACCEPTANCE.md#b03).
- [ ] [B04: People and agents have equal Browser capabilities](docs/BROWSER_ACCEPTANCE.md#b04).
- [ ] [B05: Input and navigation feel like a browser](docs/BROWSER_ACCEPTANCE.md#b05).
- [ ] [B06: Sessions recover from normal interruptions](docs/BROWSER_ACCEPTANCE.md#b06).
- [ ] [B07: One slow session cannot block the others](docs/BROWSER_ACCEPTANCE.md#b07).
- [ ] [B08: Remote Engine is an ordinary Runtime service](docs/BROWSER_ACCEPTANCE.md#b08).
- [ ] [B09: Exit has identical local and remote network semantics](docs/BROWSER_ACCEPTANCE.md#b09).
- [ ] [B10: Prove independent placement of UI, Engine, and Exit](docs/BROWSER_ACCEPTANCE.md#b10).
- [ ] [B11: Meet measured responsiveness and media budgets](docs/BROWSER_ACCEPTANCE.md#b11).
- [ ] [B12: Profiles and user state survive safely](docs/BROWSER_ACCEPTANCE.md#b12).
- [ ] [B13: Complete daily browser workflows and accessibility](docs/BROWSER_ACCEPTANCE.md#b13).
- [ ] [B14: Preserve authority, privacy, and bounded revocation](docs/BROWSER_ACCEPTANCE.md#b14).
- [ ] [B15: Updates and repair preserve a working installation](docs/BROWSER_ACCEPTANCE.md#b15).
- [ ] [B16: Release only from repeatable product evidence](docs/BROWSER_ACCEPTANCE.md#b16).

## Now

Current identities and limits are in [state.md](state.md#current-model-foundation-23-september-2026-utc).
The model builder and monitor remain paused. These are the unfinished actions,
in order; each target keeps its own acceptance evidence.

1. **J3 / MA1 / MA3 — isolated Linux 61974:** Use the copied public-artifact
   installation only. First verify a compatible, pinned local engine and its
   binary/library hashes, then read CPU, RAM, free space and hosting policy.
   Next proof is one bounded SmolLM2 run with an engine receipt. Stop if the
   engine is unavailable, identity differs, or resource/hosting limits fail.
   The dated Get and two typed runs are in
   `.audit/public-seed-gate-61974/receipt.json`; they do not prove the public
   or local `d60dc043` installation.
2. **J3 / MA1 — public Home:** Only after the isolated engine and resource
   facts pass, request exact approval for one public Marketplace Get. First
   refresh the read-only public free-space and hosting-policy preflight; the
   last 23 September reading was 11.72%, not an action-time reading. Next proof
   is a public Get receipt on the named public Runtime/provider. Stop at missing
   approval, resource pressure, artifact drift, or a Get failure. A separate
   signed-in Assistant run and manual UI evidence must then be proved on that
   public installation; the isolated run cannot close either clause.
3. **MA / AI — Owner 61965 and Consumer 61966:** Preserve the Owner's accepted
   SmolLM2 reply and the Consumer's separate installed state. Resume the exact
   pending hosted connection, service grant, Jev, sharing and negative-path
   criteria only with a named Home, current grant, owner decision and permitted
   HTTPS route. Next proof is a fresh, receipt-bound ordinary journey on that
   installation. Stop before dispatch if its Inbox decision or grant is absent;
   historical results are in the [11 September team audit](docs/audits/2026-09-11-team-sync.md)
   and [model convergence audit](docs/audits/2026-09-11-assistant-convergence.md).
4. **MA1 / MA3 / AI / CR3:** Keep signed successor catalogue, both Qwen
   acquisition orders, full Qwen distribution/benchmark, global sole-copy
   retention, frozen-provider cancellation and approved relay support open.
   Each needs its named publisher authority, source/target resources, or scope
   decision before a new proof. Stop if that prerequisite is missing. Preserve
   the original criteria, checkbox states and dated failures in the historical
   record below and the [integration audit](docs/audits/2026-09-11-integration-preservation.md).
5. **SEC1 — hosted network authority:** Keep the permanent Linux confinement,
   public HTTPS, owner grant/revocation and unknown-create reconciliation gate
   open. The temporary Mac operator route bypasses normal Inbox decision and
   grant checks while enabled; end it before a permanent-authority claim.
   Prove each path on its named installation with zero-request denial and
   recovery receipts. Stop before external dispatch without the required owner
   authority. The `/tmp` serve default and overlapping `components.json`
   `capsules`/`external` records are bounded code follow-ups, not this docs gate.

### Dated model execution evidence and original acceptance

The 23 September source candidate `a1573675` and local receipt
`.audit/local-smol-readiness-installed.json` close the current Mac owner
SmolLM2 readiness regression: the installed Runtime matches its build, System
shows the retained model as available, and ordinary Assistant selected the
exact offer and completed a new terminal reply. The signed-in Owner and Consumer
Homes remain available for human testing. This is an installed local-model
result; public Get, full Qwen, the wider installed failure matrix and release
qualification remain open. [Current source, installed and public status](state.md#current-model-foundation-23-september-2026-utc)
keeps these scopes separate.

The public-seed model gate has isolated Linux evidence. Read-only inspection
found the sealed ipfs-provider in the current public installation; the earlier
`metadata_read` attempts belong to an older artifact. An isolated Home with
copies of the current public artifacts passed a 763-byte capacity and restart
check, a fresh typed Marketplace SmolLM2 Get and admission, and two terminal
local Assistant runs across a Runtime restart. Receipt:
`.audit/public-seed-gate-61974/receipt.json`. Next, prove a compatible, pinned
Linux local engine in that isolated Home, with a read-only CPU, RAM, free-space
and hosting-policy preflight for the public seed. Record the engine binary and
library hashes and receipt, resource readings and a bounded SmolLM2 run; stop
if the engine is unavailable or the seed has resource pressure. Then seek
exact approval for one public Get and a conditional signed-in public Assistant
prompt under the private public-check plan. Public Get remains
untried on the current installation. The repo-seal no-follow and owner-only
ancestor review is separate security hardening; this gate makes no SEC1 claim.

### Model delivery R14 — 0.7.1 on public Home

Execution plan recorded 22 September 2026: complete the remaining model product
before the next human test. The reviewed five-outcome candidate below is the
regression baseline. Codex owns source, builds, installed test Homes and evidence;
one read-only reviewer checks material milestones. The coordinator owns Notion
and monitoring. Preserve all human connections, keys, conversations and drafts.

The sequence at that checkpoint was: simplify Models and Approval Lens with installed keyboard
and responsive proof; project existing shared service offers in Marketplace and
complete its ordinary access handoff; complete the installed access and failure
matrix with controlled test state; finish remaining catalogue, retention and AI
acceptance clauses; then run combined checks, review and artifact reconciliation.
A labelled Approval Lens sample uses real Decisions advice with human authority
kept separate. Generic remote failures retain unknown acceptance until an explicit
destination assertion proves refusal before dispatch. Full Qwen work and the
previously deferred relay/performance gates retain their original scope.

At that checkpoint, installed System Models and Assistant used one editor and one
selector. Approval Lens selection and the labelled sample work; reopening the
sample reuses its saved evaluation. Marketplace shows the granted offer's Home,
upstream, payer, limits and availability, and hands that exact offer to Services
or Assistant. The existing-grant path passed. A fresh request and six-hour Inbox
approval awaited human confirmation; the later hosted-sharing result is recorded
in the accepted regression evidence below.

At the 22 September checkpoint, public Home Get targeted the seed Runtime,
while the viewer's Mac kept its own separate Content and Carrier path. Five
public SmolLM2 attempts settled at `metadata_read` with zero index and
transferred bytes on the then-installed provider. That label did not identify
the provider's internal cause. The later Mac small-model journey passed; the
current public artifact and isolated Linux result are recorded in Now and
[state.md](state.md#current-model-foundation-23-september-2026-utc).
Marketplace target-Home clarity and signed-in seed-local inference remain open
product acceptance, with the next proof specified in Now.

The isolated installed hosted matrix passed destination authority, sharing
pause/resume, combined private/shared capacity, key replacement, removal,
retained-run access, transport loss, cancellation and owner restart checks.
The key check exposed a swallowed refresh error; exact key-only refresh now
preserves the running worker and gives subsequent requests the new key.
Configuration refresh failures report a pending state. Disconnect withdraws the
exact shared offer while retirement is pending and preserves other connections.
Independent review accepted this diagnostic matrix. Source checks pass for Use
retention and caller-bound Remove, including busy, foreign-claim and recovery
cases. Installed Use creates the caller's Keep; human SmolLM2 Open/selection and
reload pass with artifact and protected-state parity. Fresh access approval
and the remaining acceptance decisions stay open. The tiny held-read test found that a frozen shared provider prevents
terminal cancellation until its read drains. The reservation stays charged.
Recovery after releasing the provider passed, including restart without replay;
the frozen-provider criterion remains open. Request-exclusive read ownership is
a larger mechanism decision, separate from the existing HTTP read deadline.
The running-provider held-HTTP check passed actual socket drain at 5.002 seconds,
terminal cancellation with zero reservation, a separate small read and restart
without replay. Combined installation and final source gates pass. Trusted
publisher successor evidence and global sole-copy evidence remain open clauses.

A 23 September bounded source experiment sent a `cat` request with
`max_bytes: 763` to a test-owned provider, froze that child, cancelled the
caller, and reaped the child through bridge shutdown in 0.43 seconds. The pipe
lock was released. Independent review found no blocking defect. This proves a
small request-exclusive shutdown mechanism in the bridge harness; the fixture
did not transfer Content bytes or exercise product reservation and admission.
Production Content still uses a shared provider bridge, so shutting it down
would affect other reads. Keep frozen-provider terminal cancellation open.
Next test a request-owned Content worker or equivalent bounded cancellation
protocol with other reads kept available, then rerun the installed 763-byte
fixture and verify terminal state, zero reservation, no admission, preserved
holder, and restart without replay.
The next Mac source experiment kept a second provider bridge live while the
dedicated child was frozen. That bridge returned and verified 763 synthetic
bytes before and after the frozen child was cancelled and reaped. A test
cleanup guard reaps the child on failure. This proves process
lanes can remain independent, but does not route a product Content read to a
request-owned lane. Model preparation currently holds the shared registry;
ContentProvider holds a weak reference to that registry, `fetch` invokes its
shared `ipfs` target, and failure cleanup calls the same target for a drain.
A bridge-only change therefore cannot prove drained work or release the
reservation. Request-owned routing must bind readiness, bounded fetch, cancel
cleanup and restart reconciliation to the same owned provider process while
unrelated Content reads retain the shared process. That change crosses the
registry, Content and preparation ownership boundary; keep installed acceptance
open until this route and its failure cleanup are implemented and tested.

New bounded local-model result, 22 September 2026: the specialized
`aac6fef/laya-typed-decisions-mlx` checkpoint at revision
`f9e501c2080cc57c13d6887820329758f5351125` ran offline through the
pinned MLX engine at `0a859518634112655cb97c745dbf04f5191aaf13`.
Its weight hash matched `804ef8802b4cac7a67913b0cfb8448659e934a50284aaa867b98d7d9a6e7d1e0`.
In eight prelabelled Approval Lens cases it matched five recommendation and
three risk labels, with two unsafe `approve` recommendations, one using the
current production state shape. This small test does not estimate a general
error rate. The checkpoint stays outside Runtime. A later proof needs reviewed
labels from actual six-field states, exact Decisions v1 mapping, and offline
isolation before any product integration. Receipt:
`.audit/laya-feasibility/result.json`; read-only review accepted this boundary.

Current product correction in progress: System Models has one compact
evaluator selection and no fictional sample. A real Assistant hosted request
now keeps its prompt at the pre-dispatch Inbox gate and offers direct review
and an explicit Continue request action. Inbox names the connection, prompt
recipient, payer and continuing approval scope; System Models lets the owner
end that approval. Six focused Jev tests and the Assistant 390/768/1280 replay
smoke pass. Both signed-in Homes serve bytes that match the edited Home,
Assistant and System source. On Consumer, a genuinely new Venice request stayed
held across a Home restart and Review in Inbox opened the pending card. The
person approved that connection; its saved Jev record still has no actual
outcome. The separate existing Consumer service grant has expired. Consumer
sent one fresh request through Services, and the owner Inbox holds it for a
person's approval. The short hosted prompt, both keys, and the existing share
stay in place.

The next ready correction is a fail-closed pause on hosted HTTP. A review found
that System key validation and the native model provider could send HTTPS
without a separate owner egress decision. The installed candidate now refuses
System Validate and Save, all external model adapter dispatch, and queued
hosted workers. A diagnostic Home sent four dummy-key Validate/Save requests to
a controlled endpoint: all returned the pause error, with zero endpoint
connections and unchanged provider config. Production provider process tests
refused four adapter types on create, retry, and restart; worker tests covered
queued create, status, cancel, text, and Decisions. Both signed-in human Homes
run matching installed Runtime/provider binaries and serve matching System files.
Consumer System shows
Venice and Jev paused; owner Assistant completed a local SmolLM2 reply.
Eighteen protected files kept their hashes and inodes, and disk free space
remains 21%. Receipt: `.audit/codex-hosted-egress-containment-installed.json`.
The native provider in those human Homes still lacks OS socket confinement. A new Venice text call
waits for Runtime-owned network isolation, an egress broker, an exact owner
HTTPS grant, and fresh consent. The owner service request is a separate Inbox
decision; its approval grants service access, not provider HTTPS. Production
replay of an already-active HTTP job remains unproved for that adapter path.
Inbox approval history and revocation remain product acceptance work. Keep the
saved prompt, owner request, and original acceptance blockers below.

Mac socket-isolation milestone: commit `364d0254` makes Runtime start the
verified model-provider through Seatbelt. Its fixture got `EPERM` for direct
external TCP from both the child and a descendant; local TCP succeeded. The
installed provider binary completed a new SmolLM2 run under the same policy.
Independent review found that the first localhost exception permits a local
relay. The first Runtime build did not replace either signed-in Home. Receipt:
`.audit/hosted-egress-design-scratch/seatbelt-milestone.md`.

The next local commit, `8bd9f3ae`, narrows macOS egress to one selected TCP
port per initial local offer and denies IPv6 outbound. Child and descendant
probes refused unrelated loopback and external sockets. An installed diagnostic
Runtime and provider passed source/built/installed hash checks and Home startup;
the installed provider binary completed SmolLM2 under the policy and removed
its guard and engine after a forced kill. The human Homes and protected state
remain unchanged. Review found an unowned port interval before first inference
and stale port permission after offer removal. An exact Unix socket rule passed
child and descendant tests and selected the next private transport. Hosted
calls stay paused. The following checkpoint implements that transport;
Home-launched Runtime-to-Smol proof, HTTPS broker grants, owner consent, and
Linux confinement remain open.
Receipt: `.audit/hosted-egress-design-scratch/narrow-seatbelt-installed.json`.

Commit `f05bd171` now makes Runtime own a private Unix broker socket for each
initial local offer. The macOS Seatbelt policy lets the model provider reach
only its selected socket. The broker checks provider identity, engine ancestry,
exact llama routes, request bounds and lifetime. A source-linked Runtime test
completed a new SmolLM2 run through the diagnostic Home's installed provider
binary and closed the broker socket. The diagnostic gateway has no admitted
SmolLM2 offer, so an installed Home-launched inference on this transport still
needs proof. Its installed Runtime and provider match their built SHA-256 values;
the provider manifest check and Home startup passed. The human Homes remain
signed in, all 18 checked protected files retain hash and inode, and disk free
space is above 20%. `RUST_TEST_THREADS=4 just verify` passed 4,566 tests with
28 ignored. Default and one-thread failed runs and their classified test
interference remain in the receipt. Independent review found no new high or
medium broker issue. Next: Runtime-owned hosted HTTPS egress grants with exact
owner consent and controlled redirect, DNS, revocation and active-job proof;
then Linux kernel confinement. Hosted calls remain paused until that authority
is installed. Receipt:
`.audit/hosted-egress-design-scratch/unix-broker-diagnostic-installed.json`.

Hosted-effect broker checkpoint: local commit `5e16adaa` routes the confined
macOS model provider's hosted effects through a Runtime-owned Unix socket and
keeps credentials in Runtime storage. System Validate/Save uses the same exact
grant/destination check. Public HTTPS remains paused. Installed diagnostic
Home 61971 returned 400 with zero controlled-sink connections before a grant,
sent one dummy-key request with an exact loopback grant, then returned 400 with
zero new connections after revocation. Built/installed Runtime and provider
hashes match, the provider manifest passes, and the diagnostic and two signed-in
human Homes return 200. All 18 protected hash/inode records are unchanged;
disk free space remains above 20%. Independent review accepted this fixture
scope and found no new high or medium issue. Receipt:
`.audit/hosted-egress-design-scratch/hosted-broker-diagnostic-installed.json`.
Next: bind the owner grant to verified admin proof and the exact Runtime
run/request, bind HTTP-job status/cancel IDs to persisted create results, and
stop active System validation promptly on revoke. Only then review public
HTTPS routing and per-platform process confinement. Keep saved Venice and Jev
state, the pending Inbox request, and the signed-in Homes available for the
person's later test; this fixture made no paid request.

Admin/run broker checkpoint: source now requires an active admin passkey
principal for each private egress grant. System Validate/Save binds that grant
to its verified Home admin launch proof; model effects also require exact
run/request fields and a Runtime bridge record created from a valid typed
`runs_create` binding after the provider pipe flushes. The bridge records only
hosted offers, so local Smol runs do not consume its admission table. Narrow
source tests passed for absent run authority, wrong admin proof, wrong run
fields and exact local dispatch. Diagnostic Home 61971 runs matching built and
installed Runtime SHA-256 `fe74ea8f` and provider SHA-256 `c23d3e7e`; its
dummy-key System route denied absent/wrong proof with zero sink connections,
sent one exact-grant request, then denied after revoke with zero new
connections. The grant is inactive, all 18 protected hash/inode records match,
the signed-in Homes return 200, and disk free space remains above 20%. No paid
call was made. The first installer pass stopped on object-provider's separate
stale lockfile; a one-entry lock update let the canonical installer pass.
Independent review confirmed the dispatch gate and identified one bounded
availability limit: up to 4,096 hosted run records can remain for two hours
when completion is never observed. Next: installed model-effect denial with
an exact run/request, active validation revocation, then public HTTPS
consent/routing and Linux confinement. Public
HTTPS stays paused; this diagnostic source/installed proof is not public
activation or a completed five-outcome candidate. Receipt:
`.audit/hosted-egress-design-scratch/admin-run-installed-receipt.json`.

HTTP-job source checkpoint: Runtime now records a successful create result in
a private, bounded atomic file before returning the job ID to the provider.
Status and cancel require the exact offer, run, request, job ID, backend routes
and credential identity. An authorized retry of a recorded create returns the
same job ID without a second upstream create. Per-key reservations allow
unrelated creates to proceed while one upstream is slow. Runtime canonicalizes
the checked JSON body before forwarding it. Source checks cover missing and
wrong job IDs, route and account drift, replay without a second dispatch, and
the existing exact-grant fixture. Independent review found and verified fixes
for lock, replay, route and account-binding gaps. At this source checkpoint,
installed HTTP-job effects were still unproved. An upstream that accepts create
but loses its response before Runtime records a job ID can still leave
settlement unknown;
upstream request-ID idempotency or lookup is needed for full recovery. Public
HTTPS remains paused.

Active System validation revocation now checks the same current admin grant
every 250 ms while the upstream request or response body waits. A local
slow-header fixture received one authorized request, then revocation ended
validation before its 20-second timeout. All six scoped broker tests, the
basic source gate and a bounded read-only review pass. A slow response body
uses the same monitor but has no separate fixture proof. The canonical
source-home install and owned restart on marked diagnostic Home 61971 produced
matching built/installed Runtime SHA-256 `541890cc…`; provider SHA-256
`c23d3e7e…` and manifest verification passed. The installed slow-header
fixture sent one dummy-key request to its local sink, revoked the exact grant,
and received System HTTP 400 in 0.002 s while the sink still withheld its
response. Original grant bytes, provider config and fixture stayed intact.
The two signed-in human Homes returned HTTP 200, their gateway PIDs stayed,
and 29 protected files kept hashes and inodes. Disk remained above 20% free;
no paid call was made. Receipt:
`.audit/hosted-egress-design-scratch/active-validation-installed-receipt.json`.
Public HTTPS remains paused.

Installed HTTP-job checkpoint: marked diagnostic Home 61971 now runs Runtime
built and installed at matching SHA-256 `ccf6763d…`, with its native model
provider verified at `c23d3e7e…`. A dummy loopback sink saw one authorized
create, one status for the bound job ID, and one cancel. It saw zero requests
before a grant, for wrong-route or wrong-account grants, and after revocation.
An adversarial diagnostic run preserved then moved its provider journal entry
before an exact create replay: Runtime returned the persisted job ID with no
second upstream create. Changing only that diagnostic journal's job ID made
cancel and status receive broker 403 responses without reaching the sink. The
first fixture used an offer ID outside Runtime's hosted format; it made zero
sink requests and was corrected before this proof. All fixture-created run
journals and the job binding are now preserved under the ignored audit
directory so the restored diagnostic model provider starts cleanly. Its offer
config is absent, original fixture and grant bytes are restored, and grants
are inactive. Installed `offers_list` returns HTTP 200 with zero diagnostic
offers; all three Homes return HTTP 200. Independent review found no remaining
high or medium evidence issue in this diagnostic scope. No paid request
occurred. Receipt:
`.audit/hosted-egress-design-scratch/installed-http-job-receipt-v3.json`.
Public HTTPS consent/routing, Linux confinement, and upstream recovery for a
lost create response remain open.

Lost-response source checkpoint: Runtime now writes a private create-attempt
record and syncs its directory before sending an HTTP job create. If the
upstream receives the full request but its response is lost, a retry with the
same offer/run/request ID is denied without a second dispatch. A successful
response replaces the pending record with the exact job ID. The local sink
test checks the full received POST and zero requests on retry; the focused
broker tests and independent read-only review pass. Unknown attempts remain
in the bounded 4 MiB/4,096-entry journal and fail closed when it fills.
The installed diagnostic Home repeated this fault: the local sink received
one full create POST, closed before a job ID, and a same-ID retry sent no
second POST. The provider settled the run as `settlement_unknown`; the
private fixture was restored and Home returned HTTP 200. Installed Runtime
SHA-256 `e4a27068…` and provider SHA-256 `c23d3e7e…` are bound in
`.audit/hosted-egress-design-scratch/lost-response-installed-1616-retry/receipt.json`.

The follow-on installed restart check kept the null-job marker byte-identical
while the marked Runtime and model-provider restarted. The first terminal run
record was held in the private audit, so a same-ID retry made a fresh provider
dispatch decision. With its exact grant active, that attempt settled
`settlement_unknown` and sent zero further POSTs to the listening sink. The
fixture was restored with no live job binding or run journal; Home returned
HTTP 200. Bounded independent review found no high or medium evidence issue.
Receipt:
`.audit/hosted-egress-design-scratch/lost-response-installed-restart-1616/receipt.json`.

Upstream lookup or idempotency and explicit reconciliation of unknown attempts
remain required before public hosted use. External HTTPS remains paused; the
signed-in Homes and public seed were not changed.

SEC1 upstream qualification on 23 September kept this boundary fail-closed.
The tested `http_job_artifact` route names a controlled local fixture, not a
selected commercial job provider. Its adapter accepts create, status and
cancel URLs; it has no qualified lookup by the caller's request ID or an
upstream create-idempotency rule. The configured Venice and Jev offers use text
adapters, so their completed requests do not qualify the HTTP-job contract.
OpenRouter documents video status lookup by its returned job ID, but that ID
is unavailable when create loses its response. Its generation lookup also
requires a returned generation ID. The reviewed video API docs give no
caller-request-ID lookup or create-idempotency guarantee. OpenRouter's
response cache covers text endpoints, not video create, and does not supply
this guarantee. These published APIs do not justify a second create after an
unknown first result. The accepted
installed controlled-sink receipts above show one full POST, zero extra POSTs
on same-ID retry and after Runtime/provider restart, and durable
`settlement_unknown`. The installed known-job fixture binds status and cancel
to the original offer, run, request, job ID, routes and credential identity,
while exact create replay uses its persisted ID without a second POST. Receipt
`.audit/sec1-unknown-create-contract/receipt.json` binds the reused evidence
and official API references. A named upstream must publish and pass a
request-ID idempotency or lookup contract before automatic unknown-create
recovery can be implemented. No paid call or human Home mutation was needed.

Linux SEC1 diagnostic: an isolated C fixture on the owned Linux target passed
with a `no_new_privs` seccomp filter. Its child and forked descendant received
`EPERM` for new IPv4, IPv6, and Unix sockets, for local/external TCP connects
through an inherited unconnected socket, and for local/external UDP sends.
Both completed a synthetic local round trip through the selected preopened
Unix broker channel; the parent reaped both. Exact source, binary, and result
hashes are in `.audit/linux-sec1-confinement/receipt.json`. Independent review
found no high or medium issue in this fixture scope. This was kernel-boundary
evidence, not an installed model-provider or SmolLM2 inference result. At that
stage, the Linux provider had no confined launch; its clients used pathname
Unix sockets, and it spawned llama.cpp as a descendant that would inherit the
filter.
The next source fixture, `scripts/linux-model-seccomp-transport-proof.c`,
separates a Runtime-owned synthetic engine from an exec'd provider child.
The engine accepts on Runtime's Unix listener outside the filter; Runtime
relays two exact requests. The provider receives only connected FD 3 after
`close_range`, and its child and descendant each complete a broker round trip
while direct socket operations return `EPERM`. The isolated Linux build and
run pass, with source/binary/result hashes in
`.audit/linux-sec1-confinement/transport-receipt.json`. Independent review
found no high or medium issue in this design fixture. It does not execute the
Rust Runtime, native model-provider or SmolLM2. Next: integrate this
ownership and transport in those components and prove installed SmolLM2 on an
isolated non-public Linux target. Linux product confinement and public hosted
HTTPS remain open.

The Linux SEC1 source slice now launches model-provider through a Runtime-owned
local Unix broker with a fail-closed seccomp filter. New IPv4 and IPv6 sockets
return `EPERM` in its child and descendant test; the installed provider, guard
and llama.cpp processes report `NoNewPrivs=1` and `Seccomp=2`. An isolated,
non-public Linux Home on loopback port 61973 registered the installed provider.
The installed Runtime and provider match their built SHA-256 values
`d59bf992…` and `0b8e6c76…`; the installed SmolLM2 bridge test completed.
The exact hashes, process ancestry, socket families, HTTP checks and 10.33%
disk reserve are in `.audit/linux-sec1-confinement/installed-observation.json`.
The first Home start skipped the provider because the copied model directory
did not match preparation inventory rules; moving it into the valid model
location and restarting this isolated Home restored registration. This is
source-linked installed bridge evidence. A signed-in Assistant run through
that Home, strict preopened-channel-only engine ownership, and public hosted
HTTPS remain open. Bounded independent review found no high or medium issue
in the source slice and kept the Home Assistant claim open.

Mac hosted route approval has source and installed diagnostic proof. Runtime
records exact external HTTP(S) owner decisions in private state, presents them
in Inbox, and checks active decisions and run-bound grants before and during
broker dispatch. System can end saved-route and key-check approval. A dummy
loopback sink saw zero validation requests before approval, one after the real
admin-passkey Inbox action, and zero additional requests after System End.
The diagnostic Home was restored to its original binary and private manifest;
the two signed-in human Homes stayed open. Receipt:
`.audit/sec1-egress-installed/installed-sink-result.json`. Next: integrate
the Linux confinement fixture into the production provider path, then prove
the installed SmolLM2 and hosted routes on each target. Keep public HTTPS
paused until its separate owner and target proofs pass.

Owner Inbox route history and exact End are now checked on the isolated Mac
diagnostic Home. Its private decision file supplies current dispatch authority;
expired records are retained in write-once private history files. Inbox shows
Pending, Approved, Denied and Ended route facts to the admin passkey owner,
with a keyboard-operable End action. A fresh validation after installed Inbox
End made zero new requests to the controlled sink. Built/installed Runtime,
source/installed/served Inbox hashes, browser keyboard actions and exact
restoration are recorded in `.audit/sec1-inbox-history/`. The prior diagnostic
Runtime and Inbox file are running again, and both signed-in human Homes remain
open. Next: Linux product confinement, upstream unknown-attempt reconciliation,
and separate public HTTPS owner/target proof; wider MA/AI/CR acceptance remains
open.

Mac SEC1 destination challenge is complete for the installed diagnostic Home.
The broker binds a private test CA to the owner decision and uses it as the
only TLS root for an exact loopback HTTPS route. A controlled redirect did not
reach its second sink, a `localhost` hostname substitution was refused before
dispatch, and a wrong-name certificate signed by the approved test CA sent no
HTTP request. The exact approved HTTP and HTTPS routes each worked. The
diagnostic Runtime and private state were restored; the signed-in human Homes
remained open. Source commit `623affc3`, installed artifact parity, sink counts,
restoration hashes and independent review are recorded in
`.audit/sec1-destination-boundary/receipt.json`. Next: Linux production
confinement, upstream unknown-create reconciliation, public-hostname DNS
answer rebinding, public CA routing, and separate public HTTPS owner/target
proof; wider MA/AI/CR acceptance remains open. External HTTPS stays paused.

The isolated Mac SEC1 DNS challenge now has installed proof for an approved
`localhost` HTTP route. A controlled resolver returned `127.0.0.1`, switched
its answer to `::1` before dial, and the HTTP client sent only to the pinned
IPv4 sink. With `::1` as the next answer, the broker returned 400 before any
new request; restoring `127.0.0.1` let the same approved route work again.
The corrected final check counted two IPv4 TCP accepts and zero IPv6 TCP
accepts; its private resolver trace records a successful `::1` answer in the
live gateway. The earlier HTTP-only count and first trace remain preliminary
evidence. Source commit `dfc1a945`,
built/installed artifact parity, exact sink counts and byte-for-byte diagnostic
restoration are bound in `.audit/sec1-dns-rebinding/receipt.json`. Public DNS
and CA routing, Linux production confinement, unknown-create reconciliation,
external HTTPS activation and real hosted acceptance remain open.

Consumer hosted-model acceptance remains open. Its saved Jev advice for the
Venice connection was `defer` at medium risk with 72% reported confidence; the
admin approved the connection, but two later Assistant runs stopped before
hosted HTTPS dispatch. The Jev record proves prior advice and human approval,
not a fresh Jev call or a successful model response. A Mac Runtime broker
source candidate `71b4ea79` now permits only the fixed Venice/OpenRouter validation,
chat, and Decisions routes after an exact owner Inbox decision, with public DNS
pinning and normal TLS roots. Sixteen broker tests and bounded independent
read-only review passed with no P1/P2 finding. These source checks do
not yet prove installed public HTTPS, revocation, or a completed Venice run.
Keep the two signed-in Homes and their keys intact; use a separate owner action
and installed target proof before external HTTPS activation.

Guest-hosted setup is a separate release blocker. System currently shows Add
to a signed-in guest even though the provider routes require the Home admin;
the provider config and secret store are Home-global. Preserve that admin
boundary until a verified guest principal can own its key, offer, dispatch,
Inbox decision and revoke in private principal state. Prove on two installed
principals that the other principal, including the admin UI, cannot list, use
or change the guest key or offer, and that one guest's revoke affects only its
own runs. An anonymous Home visit has no recoverable private owner. Hide the
unusable Add path until guest authority is implemented and proved.

Remote-local SmolLM2 sharing needs an offer-bound grant before a demo claim.
`gateway_model_service.rs` stores the existing peer grant by provider and
requester principal, without an offer ID. Its next offer list and run request
re-read the current set of shared local models. Consumer already has a live
generic AI-model grant for Owner's shared hosted Venice offer, so selecting
`Share my AI model` on Owner would add SmolLM2 to that same grant without a
separate SmolLM2 Inbox request. The read-only preflight stopped before Share;
both shares and the existing grant stayed unchanged. Bind a grant to each
approved model offer, preserve or migrate the current Venice grant explicitly,
then prove fresh Smol request, owner approval, run and revoke on two installed
Homes. The fresh ordinary local SmolLM2 Assistant run on Owner Home passed;
remote-local SmolLM2 is still open.

Source-home release stamp correction: the public demo cutover exposed five
unchanged preexisting Linux providers whose installed binaries retained their
old bytes while setup replaced their checksum, size and CID pins with empty
source fields. Source-home stamping now carries the previous pin only for
operator-drive-adapter, drm-provider, rights-provider, key-provider and
decrypt-provider, after the installed binary matches the prior SHA-256 and
size and the source/installed path identities agree. A changed binary or
stripped prior pin stops the stamp without replacing the manifest. Kubo keeps
its separate archive pin path. The focused inventory smoke, shell syntax,
basic gate and bounded read-only review pass on source. This has no Linux
installed claim: before a later release, verify the prior manifest against an
independent receipt, then prove the canonical setup and all five provider
verifications on an isolated Linux Home. The public seed's separate owner
already repaired its live pins and remains outside this source test.

Accepted regression evidence for the original five outcomes:

1. **CR1/CR2 prerequisite closeout — verified:** source lineage and installed
   direct-only acquisition are bound. Shared endpoint construction enforces the
   same policy for listeners, short-lived dials and bind fallback. Eleven policy
   checks and six affected fixture checks passed. Admission retains its defined
   cache quota. The reverse Mac dial and CR3 relay support remain separate gates.
2. **MA1/MA3.2 small-model journey — verified:** a fresh source-built Mac Home
   obtained the complete signed SmolLM2 closure through ordinary Marketplace
   and Carrier from the seed. Two new Assistant runs completed. Reload and an
   owned consumer restart preserved conversation, selection and an unsent draft.
   Warm reopen with the seed route withdrawn kept the same weights and one quota
   charge, with zero Carrier payload traffic. The human-created passkey session
   remained signed in. Independent review accepted the installed journey.
   Marketplace's three-minute polling limit required Refresh during the longer
   transfer; Runtime continued the same operation. The updated installed UI now
   polls until a terminal result or view closure; the regression check passed
   131 polls and verified cleanup. Full Qwen and public installer qualification
   remain outside this proof. The final installed candidate also completed a
   fresh SmolLM2 prompt in ordinary Assistant; the original journey retains
   the cold delivery, restart and zero-payload evidence.
3. **MA4 live Venice and Jev — verified:** ordinary Home configured private
   OpenRouter and Venice instances. A new Venice Assistant request completed
   with `qwen-3-8-flash`; its receipt binds visible input, named offer, request,
   run and terminal output. Provider usage and billed cost remain unknown.
   Real Jev evaluation now completes through the typed Decisions adapter and
   appears in Inbox. The catalog pins the exact provider response identity;
   strict typed output and provider-reported accounting remain separate.
   The live advice recommends defer with medium risk and 56% confidence.
   The human approved in Inbox, then ordinary Assistant resubmitted the calm-river
   prompt. DS4 completed with a matching input hash, named instance, request,
   run and terminal reply. The receipt keeps the connection decision separate
   from create acceptance and terminal inference. Human authority remains intact.
4. **MA4 remote hosted use — bounded handoff verified:** two signed-in Mac Homes
   completed ordinary contact, service request and human approval through Home.
   The consumer listed the authorized offer before paid inference, selected the
   named Venice instance, and received a completed reply. Its request, grant,
   remote offer and input hash match the owner run and provider journal.
   Assistant now names the provider Home as payer and identifies both prompt
   recipients; upstream retention, usage and billed cost remain unreported.
   The completed conversation and remote selection survived reload. The consumer
   holds zero provider keys; all nine owner connections and key hashes remain.
   Three mailbox tests and 29 remote-model tests passed, including exact fresh
   authority denials, pause/disconnect withdrawal, regrant and old-run access.
   One private/remote concurrency test passed. Replacement, cancellation and
   loss settlement reuse their named passing source fixtures. These checks keep
   their source-test scope. An authorized installed pause and fresh request
   produced no new owner run or provider journal. The completed run remained
   intact; restoring the exact share preserved all keys and the existing grant.
   A new chat and explicit model refresh recovered the remote offer without
   another paid request. The consumer conservatively retained unknown acceptance;
   precise refusal copy needs an explicit destination pre-dispatch assertion.
   The full installed negative matrix remains unclaimed.
   This is a local two-Home path. Marketplace projection of service offers and
   access remains a separate product flow to complete.
5. **MA5.2 combined candidate — local handoff verified:** the earlier combined
   candidate passed the basic AGENTS gate and `just verify`, including workspace lint/tests, all separate capsule
   workspaces and the Browser local-exit helper. The server suite passed 2,186
   tests with 16 opt-in skips; the model provider passed 215 unit tests and five
   process tests with two opt-in skips. Other live IPFS and documentation skips
   retain their recorded reasons. Independent review accepted the source,
   artifact evidence and local commit boundaries. Focused checks cover the later
   repairs. Runtime, provider and changed capsule hashes match both installed
   Homes; served Assistant disclosure bytes match the reviewed source. The
   reviewed local commits preserve the original dirty work and parent history.
   This candidate is the regression baseline for the active product revision;
   publication and wider qualification retain separate gates.
   The initial combined run exposed stale authorization, key-permission,
   direct-discovery and reconciliation fixtures. All 25 affected cases now pass
   after independent review. Optional
   `carrier_bind_addr` keeps a configured direct bootstrap listener stable
   across restart and fails closed when the explicit address cannot bind.
   A later parallel run exposed a paused-clock race in two reconciliation
   fixtures. Both now keep virtual time under explicit test control with a
   real-time bound; all 13 reconciliation cases passed together, with their
   ownership assertions intact.

Full Qwen transfer/copy/benchmark work remains deferred. Indefinite provider-hang
cancellation retains a separate open clause; the installed controlled-drain test
is accepted only within its scope. CR3 is required before claiming relay support,
not before a working direct journey. User-owned local cleanup, the 10 percent
disk reserve and all human Homes/holders remain protected. Publication and public
deployment require separate approval of the exact candidate.

The following nine-card detail is the historical acceptance/evidence inventory,
not a competing execution order. Its dated statuses do not assign a current
worker or supersede the sequence above. Keep the original MA/CR obligations.

| Order | ID | Status | Result and remaining clauses |
| --- | --- | --- | --- |
| 1 | MA5.1 | Done | Alignment and install docs match Linux x86_64/aarch64 and macOS Apple silicon. `python3 scripts/install-bootstrap-test.py --bash /bin/bash` ran 53 tests, 4 skipped publisher fixtures, 45.561 s. Receipt `.audit/ma5-1-installer-bootstrap-receipt.json`. This card does not close final MA5. |
| 2 | CR1 | Done | Source: unset and `direct` start Isolated; `public` is an error; Isolated strips and rejects foreign relay hints before MemoryLookup and connect; default listener is Isolated. Receipt `.audit/cr1-one-network-policy-receipt.json`. The closeout rebuild had dropped that dirty patch, so canonical `carrier.rs` is repaired from the reviewed selector. Installed proof Home `127.0.0.1:61964` SHA-256 `16fb94ad…` logged isolated and published 6 IP addresses and 0 relays. Receipt `.audit/ma3-2-carrier-policy-receipt.json`. Remaining: Public N0 stays a test constructor; approved relay is CR3. |
| 3 | CR2.1 | Done | Isolated Mac 61955 `elastos` SHA-256 `f585f1b6…` and Linux holder 61954 `86ed4877…`. 8 MiB DirectOnly 128 reads 44.11 s, SHA-256 `e6d36653…` match, one IP endpoint UDP 37254, zero relay addrs. Receipts `.audit/cr2-1-installed-baseline-receipt.json` and `.audit/cr2-1-wan-8mib-transfer-receipt.json`. |
| 4 | MA3.1 | Done | Source Isolated dest copy 8 MiB 95.14 ms. Installed WAN dest copy 8 MiB 2.059 s and 64 MiB 4.524 s, SHA-256 `e6d36653…` / `f8550531…` match, UDP 55180, zero relay addrs. Qwen forecast from the 64 MiB sample is 415.9 s with 3184 s headroom inside 3600 s. Dest-path cancel, silent-peer idle, truncated size mismatch and explicit retry passed in `test_content_dest_stream_cancels_fails_over_and_retries` 30.38 s. Installed dest retry hashed; a bad `expected_size` was rejected. Receipts `.audit/ma3-1-source-stream-to-disk-receipt.json`, `.audit/ma3-1-wan-8mib-dest-stream-receipt.json`, `.audit/ma3-1-wan-64mib-dest-stream-receipt.json`. Holder still cats the object into memory; MA3.2 owns full Qwen delivery. |
| 5 | MA1 | Active | Sub-gates replace the monolithic status. Accepted 12. Waiting 2. Needs one named proof 1. Receipt `.audit/ma1-acceptance-matrix-receipt.json`. 1.1 Current signed catalogue and unchanged Refresh: Accepted. Isolated 61953 Refresh re-read catalog SHA-256 `c81d4357…`, both CIDs, verified, zero `content.use`, admitted bytes unchanged. Receipt `.audit/ma1-61953-trust-refresh-receipt.json`. 1.2 Signed successor catalogue: Waiting on a catalog signing key. Receipt `.audit/ma1-catalogue-successor-verify-receipt.json` records signature present on the current catalog and `signing_key_in_tree` false. 2.1 Cancel: Accepted. Isolated 61956 first Smol Get `f9139155…` cancelled at 0 bytes. Receipt `.audit/ma1-61956-prep-cancel-compact-receipt.json`. 2.2 Retry admit: Accepted. Later retry admitted `174da280…` at 144,835,448 bytes. Receipt `.audit/ma1-61956-retry-after-cancel-compact-receipt.json`. 2.3 Live window: Accepted as a failed read inside `created_at+3600` with 0 bytes and the clock unchanged. Receipt `.audit/ma1-61956-live-window-installed-receipt.json`. 2.4 Expiry recovery: Accepted as recovery of a complete hashed stage, not an extension of the live budget. Operation `fdf972f7…`, `expires_at` 1789992106, restart 2 seconds later, `extended` false, weights SHA-256 `c4a3dd03…`, codesigned `elastos` SHA-256 `4a7a26e2…`, run `run:sha256:21ca123c…` request `842d7afa-ab0b-4098-a092-41ac2a656d2e`. Receipt `.audit/ma1-61956-expiry-parent-receipt.json`. 2.5 Interrupt restart: Accepted on the renamed `admitted-*` path. Operation `a086a940…`, same weights, budget not extended, codesigned `elastos` SHA-256 `9fa10cbb…`, run `run:sha256:3eecf583…` request `75e4bc70-95bb-4dbf-85af-74676b5adad3`. Receipt `.audit/ma1-61956-recovery-parent-receipt.json`. 2.6 Both acquisition orders: Needs one named proof. Action: admit Smol then Qwen on one disposable Home, and Qwen then Smol on another, each to `dispatch_ready`, then restart with zero extra payload. Expected result: both orders keep the signed catalogue bytes. Blocker: the Qwen order is a multi-gigabyte transfer. This card does not download Qwen. 3.1 Selection and execution: Accepted for Smol on Isolated 61953. Offer `model:68d664d4…`, run `run:sha256:49bd09ab…`, terminal output present. The receipt has an empty request id. Receipt `.audit/ma1-61953-coexist-execution-receipt.json`. The 61956 retry Ping timeout stays outside this acceptance. 3.2 Stop: Accepted. Isolated 61953 reports `settlement_unknown`, UI Outcome unknown, and `backend_report` null. Isolated 61942 reports terminal completed. The criterion allows confirmed settlement or honest unknown. Receipts `.audit/ma1-61953-stop-settlement-compact-receipt.json` and `.audit/ma1-61942-stop-settlement-compact-receipt.json`. 4 Coexistence, removal, and byte reuse: Accepted. Ordinary Remove reclaimed Smol CID `bafybeidy5kfvqwg…` and kept Qwen `2d690d11…` at 6,169,366,387 bytes. Restore `a9e6bd91…` reused 144,835,448 bytes and reached `dispatch_ready`. Qwen run `run:sha256:c5bf6a5f…` request `bc2df4bb-d648-4249-878e-b09dc57b378c`. Installed `elastos` SHA-256 `8b00e615…`. Receipt `.audit/ma1-61953-removal-isolation-journal-bind-receipt.json`. 5 Unavailable content: Accepted. The same removal left the Smol conversation on Chosen model unavailable with the Ping user turn preserved and the Qwen offer live. 6.1 Active-run removal consent: Accepted on disposable Isolated 61956 product admission `2eb4c281…`. Pointer Remove during the reply returned HTTP 409, showed Stop the current reply in Assistant, then try Remove again, left retirement none, and kept weights SHA-256 `c4a3dd03…`. After Stop, Remove reclaimed that admission. Reload showed Removed from this device. Codesigned `elastos` SHA-256 `9fa10cbb…`. Inject is false. The earlier HTTP 200 `withdrawal_pending` on overlay `8b00e615…` stays the first failure, not this acceptance. Receipt `.audit/ma1-61956-ordinary-busy-remove-parent-receipt.json`. 6.2 Local retention consent: Accepted. The same confirm copy says prepared files leave this device and conversations stay. It does not say this device holds the only copy. Receipt `.audit/ma1-61953-honest-copy-bind-receipt.json` binds the fuller sentence: other copies, if any, keep their files. Installed JS SHA-256 `0af0c7fe…`. 6.3 Global sole copy: Waiting. No retention fact can prove that no other copy exists. The 23:28 only-copy sentence is superseded and is not acceptance. |
| 6 | MA3.2 | Partial | Isolated 61954 recursive Qwen pin retained after holder restart. WAN dest copy of `weights.gguf` 6,169,341,984 bytes in 1092.804 s, SHA-256 `d784ce9e…` match. Darwin 61953 inventory `2d690d11…` admitted 6,169,366,387 bytes. Original UI Ping receipt `.audit/ma3-2-qwen-assistant-ping-receipt.json` stays partial first-delta (53 characters, 1508 ms); journal `run:sha256:7f792ef3…` completed 343 characters. Cold Isolated 61957 ordinary Marketplace Get `b280a1b9…` admitted 6,169,366,387 bytes, weights SHA-256 `d784ce9e…` match, `dispatch_ready` true, offer `model:00c7b9dd…`. A new-request-id reuse alias `a407e010…` failed in preparation without a replacement download. Startup then skipped model-provider with `engine parent protection is invalid` until the private install chmod on the llama.cpp bundle was restored to 0500/0400. Repaired Assistant Ping on 61957 completed in 22763 ms with 520-character terminal output, run `run:sha256:e7ad986b…`, request `3c3e05aa-5fca-4ddd-b764-bb2384957104`. Receipts `.audit/ma3-2-61957-qwen-marketplace-get-receipt.json`, `.audit/ma3-2-61957-qwen-weights-hash-receipt.json`, `.audit/ma3-2-61957-qwen-dispatch-ready-receipt.json`, `.audit/ma3-2-61957-qwen-assistant-ping-receipt.json`. Mac holder 61700 still has no Kubo child. The managed Home Content runtime attaches to the existing 61953 Content repo. One Kubo holds that repo lock. Its parent is the Content ipfs-provider. CID `bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi` stayed recursively pinned after that Kubo restart. Authenticated Content listed the signed closure. It read 4096 bytes and then 65536 bytes of `weights.gguf`. SHA-256 `a33a3bdc…` and `c47240f7…` matched the admitted file. Carrier availability read the same ranges. The holder ticket has two private addresses and zero relay addresses. The seed has no on-link route to those addresses. Seed holder 61954 already stores this closure, so the requester was a separate DirectOnly consumer, node `fc3c1262`, with two public addresses and zero relay addresses. The Mac Content runtime connected outbound to that ticket. The seed then read 4096 bytes in 0.601 s and 65536 bytes in 1.038 s through ordinary Content and Carrier. SHA-256 `a33a3bdc…` and `c47240f7…` matched the admitted file. Availability policy was `carrier_provider_invoke`. During the 4096-byte read, 10 UDP datagrams used the consumer's published ports, and the longest was 1452 bytes. The seed consumer repository stayed 136 KiB. Free space stayed above the 10 percent floor. The managed child repo stayed 285,892 KiB. A fresh DirectOnly seed Home at `127.0.0.1:61962` sent one Marketplace `content.use` for CID `bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi`. The three-copy charge stayed above the 10 percent floor. Mac holder node `3ddc07c6` connected outbound to managed Home node `b043677c` in 0.199 s. The preparation read used the gateway carrier. That carrier joined direct gossip with 0 bootstrap peers. The local offline read of `_elastos_object.json` returned HTTP 500. Operation `bf2ff7d0…` stopped at `metadata_read`, provider error kind Provider, 0 completed bytes, and `cancel_requested` false. `content.cancel` stayed declared. The attempt sent 18 public UDP datagrams and 0 datagrams to seed holder 61954. The gateway repository stayed 128 KiB. Assistant execution remains unproven for this Linux consumer. A later Marketplace `content.use` on the same disposable gateway, operation `2feb233d8dea49d7e67873a8a850335693c03b47e6438b1e772747ab2d293c9a`, is the owned transfer. It is not `bf2ff7d09a8da9ed29cf9dcecbdc8157218b008e3ea8cc4a2f645c68e03c69b2`. The actor is marketplace `content.use` on `elastos.marketplace.catalog`. The managed Home listen was `127.0.0.1:44117`. CID `bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi`. `content.cancel` returned HTTP 200 and set `cancel_requested` true while the journal stayed `preparing`, `completed_bytes` 24403, and `reserved_bytes` 18516684377. The worker stayed in `cat_to_path`. The last gateway warning was 2026-09-21T22:38:27Z at elapsed 900 s, then the log stayed silent. That gateway was wedged. One SIGTERM to pid 910306, process group 910306, stopped the disposable consumer within 15 s. Seed holder 61954 stayed. The Mac holder, its Kubo, and the human Homes stayed. After the stop the journal is `uncertain`, `failure_phase` `weights_read`, `cancel_requested` true, and `reserved_bytes` 18516684377. The capacity reservation is still held. This stop is operational containment. The running worker did not reach settle during that process. One normal restart of the same installed gateway, pid 955407, binary SHA-256 prefix `cc77e40279d5e595`, size 115106048, then one `content.status` for `2feb233d8dea49d7e67873a8a850335693c03b47e6438b1e772747ab2d293c9a`, wrote `cancelled`, `cancel_requested` true, `reserved_bytes` 0, `completed_bytes` 24403, and `failure_phase` `weights_read`. No `admitted-` directory exists. The worker lock was free. Recovery removed the stage weights file. The ipfs-repo grew 1,759,775 bytes and logged no `cat_to_path`. The Mac Qwen pin stayed. Seed holder 61954 stayed. Managed Home `127.0.0.1:44117` did not listen, and no second serve was started. The status HTTP body still said `uncertain` before the worker finished. SmolLM2 stays unstarted. Mac free space was 67,666,980 KiB of 482,746,452 KiB (14.02 percent). Seed free space was 44,790,288 KiB of 314,748,412 KiB (14.23 percent). No agent deletion followed the stop. Installed elastos SHA-256 `cc77e40279d5e595b8690f982c86517a0f0b8297e9c78390d6d0542fe853a4c1` matches preparation.rs at `20ac3f628`, where the stop message is line 1075. That revision and current source read model bytes with `fetch_model_part` in 64 KiB bounds. The installed binary contains `ipfs-provider cat_to_path left no dest file` and `carrier-content-fetch`. Both strings are absent from `20ac3f628` and from HEAD. The live `cat_to_path` read was that overlay. Current preparation ends a held bounded read when `cancel_requested` is set, then the existing drain and `settle_failure` path releases the reservation. `model_preparation_actual_fetch_cancel_waits_for_held_read` used a 16-byte GGUF fixture, cancelled while the weights read stayed held, and reached `cancelled` with `reserved_bytes` 0, no stage, and no admission within 5 s. Two status calls and one fresh owner status left that record in place and started no worker. The same run passed the silent-holder cancel test and the restart drain test. Disk before that run was 67,613,820 KiB free of 482,746,452 KiB. The source test passed. The disposable seed lane then ran a debug elastos from HEAD `e387c9ad` plus the uncommitted fetch helper. Built, installed, and the running process share SHA-256 `6cc2ab44ecc46b28571c669510b8e39394d223fa0219ba18d52305de1d0187c3`. The file size is 454001624 bytes. Proof gateway `127.0.0.1:61963` used a 763-byte fixture. The harness returns the ipfs-provider child when Kubo is still absent. One `content.use` then observed `op=cat` on that bridge. `kubo_pid` was null at that identification. The proof stopped the provider after `op=cat` was sent. `content.cancel` ran while the read stayed open. During the hold the journal stayed `preparing` with `reserved_bytes` 8587505. Time from the cancel request to `cancelled` and `reserved_bytes` 0 was 1.29 s. That interval includes the deliberate SIGSTOP hold of 1.389 s. After SIGCONT the terminal state arrived in 0.201 s. The cat settled before `runtime_prepare_backend`. This result is a controlled drain-order proof. The provider resumed after the bounded hold. The stage directory is absent. The admission directory is absent. Two status calls and one restart status matched and sent no new `op=cat`. The loaded linux ipfs-provider matched its manifest checksum. The gateway log shows carrier online with 1 relay. Canonical `carrier.rs` starts `CarrierNodeNetwork::Public`. `ELASTOS_CARRIER_NETWORK` is absent from that function. Parent accepts this local cancellation result for the controlled drain, the zero reservation, and the idle restart. An indefinite provider hang stays a separate gate. WAN Carrier stays a separate gate. The reviewed CR1 selector is back in canonical `carrier.rs`. Unset and `direct` start Isolated. `public` is an error. `ELASTOS_RELAY_URL` on that path is an error until CR3 names an approved ElastOS relay. Eight policy tests passed. Installed proof Home `127.0.0.1:61964` SHA-256 `16fb94ad…` logged isolated and published 6 IP addresses and 0 relays. The configured Mac holder ticket had 0 relays and 2 IP addresses. That connect timed out at `metadata_read` for operation `059969c3ba8f`. The seed holder already pins SmolLM2. Operation `8f885ed89d08` read the 726-byte signed index and admitted 144,835,448 bytes. Weights SHA-256 `c4a3dd03…` match at 144,811,072 bytes. The admitted journal still records `reserved_bytes` 443091560. Holder 191661 and lane 1004338 stayed alive. Proof gateway pid 1040783 stays up. Qwen stayed deferred. Mac free space is 121731864 KiB of 482746452 KiB. Seed free space is 40507532 KiB of 314748412 KiB. Receipts `.audit/ma3-2-mac-holder-range-proof-receipt.json`, `.audit/ma3-2-mac-holder-64kib-range-receipt.json`, `.audit/ma3-2-seed-to-mac-route-receipt.json`, `.audit/ma3-2-cold-consumer-receipt.json`, and `.audit/ma3-2-qwen-cancel-closeout-receipt.json`, and `.audit/ma3-2-carrier-policy-receipt.json`. This result does not close the mission and does not close the MA2 Ask path. Full Qwen distribution acceptance stays open. |
| 7 | MA2 | Partial/Waiting | Live grant `7a1011e23d5c1b4c` versus revoked disposable `04904686402e9fc5`. Release `elastos-server` remote_model tests passed. Receipt `.audit/ma2-remote-model-lifecycle-source-receipt.json`. Installed grant expiry passed on seed 61942: Services shows Expired, `offers_list` has only local SmolLM2, and `runs_create` on `7a1011e23d5c1b4c` returns HTTP 500 denied. Receipt `.audit/ma2-61942-grant-expiry-receipt.json`. Installed expiry stays accepted. Isolated 61942 overlay `elastos` SHA-256 `466adf88…` remembers live dest `8d03e7b1` for contact `3c20b877…`. Ordinary Ask then returned 503 "Services peer operation deadline" after Isolated connect to Mac 61680 UDP 50026 exceeded 20s. Dest ticket still lists one public relay; DirectOnly strips it. 61680 runtime log has no inbound Carrier line at the Ask time. Wallet stayed untouched. Mac services runtime node `8d03e7b1` connected outbound to DirectOnly 61942 node `ede679a3` in 0.236 s. The dialed ticket had two public addresses and zero relay addresses. `list_peers` then showed that one peer. The Mac ticket relay was left unused. This proof sends no Ask and leaves service delivery open. Receipts `.audit/ma2-ask-contact-route-receipt.json` and `.audit/ma2-mac-outbound-services-route-receipt.json`. Then Stop/pause/Busy/owner-failure on a live grant. |
| 8 | MA4 | Active | Independent move from blocked MA1, not mission completion. The one-form System Models OpenRouter/Venice shortcut is not the product boundary. MA4 uses the generic model-provider capsule and repeatable owner-bound instances. Each instance has a stable offer identity, display name, adapter, selected model, Runtime secret reference, privacy and share policy, limits, and lifecycle. One Home can keep several instances of the same provider, for example Jev via OpenRouter, private DeepSeek via OpenRouter, and shared Venice. Private is the default. The capsule UI is Add hosted model, Name, Provider, API key, Model, Test, and Save. After save the same card exposes Use in Assistant, Share as service, Replace key, and Disconnect. Share reuses Services grants, provider terms, Carrier, and the run journal. The consumer receives an authorized model service. The secret stays in Runtime-owned storage on the provider Home. Assistant and Marketplace list every typed offer independently. System supplies generic installed-capsule and secret management. Migrate existing `model:openrouter` and `model:venice` configuration without losing secrets. Terminal, JSON, and key-file workarounds are rejected. Qualify exact model and provider terms before Share (OpenRouter Terms 5.1–5.2; Venice TOS 7.3 End User API terms). Commercial billing/staking and wider providers stay Later. Historical Isolated Darwin 61958 one-form Settings fixture remains shortcut evidence only. Isolated Darwin 61960 Assistant composer truth is accepted at 1280, 768, and 390 CSS pixels: Message Assistant, no Think chip, trigger name Jev or Venice, row subtitles OpenRouter · hosted and Venice · hosted, expanded processor/destination/payer/limits/privacy/availability, keyboard selection. Receipt `.audit/ma4-composer-truth-ui-receipt.json`. Isolated Darwin 61960 installed this dirty candidate and proved ordinary Home Inbox shadow through a local Jev-compatible provider fixture. Pointer Approve kept the person as the authority. Venice recorded recommendation approve, Alpha recorded unavailable for a malformed typed reply, and Beta recorded unavailable for provider failure. One request id per offer links the sanitized recommendation, human_decision approve, and actual_outcome accepted. Records are mode 0600. Built unsigned elastos SHA-256 `4cae1446…` matches installed unsigned; codesigned SHA-256 `40b75856…`. Inbox index SHA-256 `c5d8c454…` and agent-live SHA-256 `988cd17a…` match source. Fixture eval_count is 3. Receipts `.audit/ma4-61960-jev-overlay-receipt.json` and `.audit/ma4-61960-jev-inbox-fixture-receipt.json`. This is installed fixture evidence. Remaining acceptance: live TypeSafe/OpenRouter Jev receipt; live Venice; a second named OpenRouter instance such as Jev on a live key; a second-Home remote run; CR3. Isolated Darwin 61960 fixture Home proved the instance boundary. Isolated Darwin 61958 overlay-matched that candidate, migrated the live OpenRouter key into `secrets/model_openrouter` mode 0600, and completed a private Assistant Ping on offer `model:openrouter` as journal `run:sha256:9c0b8c5b…` request `8cf4d28c-f123-4629-9c3a-fae0ba8afc56` with 122-character terminal output. The first pointer harness used `[data-agent-message]` and missed the visible Pong. The live 61958 screenshot showed false composer copy (`Ask on this machine`, Think chip, `OpenRouter · cost unknown`). That composer surface is now part of MA4 acceptance. Codesigned `elastos` SHA-256 `fe56eb48…`, `model-provider` SHA-256 `d468e245…`. Receipts `.audit/ma4-instance-pointer-ui-receipt.json`, `.audit/ma4-instance-installed-receipt.json`, `.audit/ma4-61958-live-secret-migration-receipt.json`, and `.audit/ma4-61958-live-openrouter-assistant-installed-receipt.json`. Hosted HTTP classes passed. Jev `human_decision` unit fixtures persist a skeleton shadow record only. They do not call Jev or show Inbox. UI and backend private fixtures proceed without a live key. Independent parent re-run 2026-09-21T02:00:00Z: 9 `elastos-server` `ai_provider`/`hosted_hint` tests passed, 2 model-provider hosted tests passed, `node scripts/home-entropy-check.mjs` PASS, `node scripts/home-shell-bridge-smoke.mjs` PASS. Hosted Share source fixtures passed after a parent compile repair: 13 `elastos-server` `ai_provider`/`shareable_offers` tests on existing `target-build`. Guest Share is 403. Share without the matching terms ack is 400. Explicit OpenRouter Share lists `model:openrouter` and leaves Venice private. Key replace keeps `share.enabled`. Disconnect removes that hosted offer from the shareable set. Receipt `.audit/ma4-hosted-share-source-receipt.json`. Receipt `.audit/ma4-private-source-fixture-receipt.json`. Installed Isolated Darwin 61958 Settings fixture: the CDP receipt `.audit/ma4-installed-settings-fixture-receipt.json` has `ordinary=false` and does not close ordinary Settings. A later pointer run on the same Home used Playwright frame locators for add both providers, private save, OpenRouter Share after `openrouter-5.1-5.2+model`, replace OpenRouter, and disconnect Venice. Services `model:openrouter` returns `share_enabled` true with `status` configured, matching `config.json`. Unauth GET HTTP 403. Codesigned `elastos` SHA-256 `f1ca8e5c…`. Receipts `.audit/ma4-settings-pointer-ui-receipt.json` and `.audit/ma4-settings-pointer-parent-verify-receipt.json`. DirectOnly second-Home Ask is recorded once: gateway pid 19345 logs `carrier: online` with 1 public relay on UDP 56996; Isolated DirectOnly is test-only; CR3 stayed off; Home 61959 was not created. Live OpenRouter and live Venice each wait on that provider's Home-entered key. Jev stays on TypeSafe/OpenRouter. Presence check found no hosted `config.json` and no nonempty hosted key on 61680 or public live 8090. Secrets were not printed. Receipts `.audit/ma4-live-credential-presence-receipt.json` and `.audit/ma4-venice-api-facts-receipt.json`. |
| 9 | MA5.2 | Active | Combined candidate Goal 1 source gates passed on this dirty tree. Reviewed PR #69 `7285cba` (`origin/fix/alignment-two-platform-installer`, tree `233191a6`) and PR #68 `4f7d863` + `a9d0b43` (`origin/feat/assistant-dock-mark-0.7.1`, tree `08bdc0a9`) against dirty HEAD `9a922faa` without cherry-pick. Public truth is signed release-head 0.7.1; served installer help says `Release lookup: Linux x86_64/aarch64 and macOS Apple silicon.` Semantic reconcile of #69 kept mutation-tested fail-closed installer rules, shared Browser protocol 2.1 authority, and sign-in wording. README, Getting Started, and INSTALL stay on the two-platform endpoint. Linux-preview doc rules stay out. Extra retained checks: `home --browser` and `arm64) ARCH="aarch64"`. #68 stays a later presentation slice after MA4 Assistant work. Dirty overlap is only `scripts/home-entropy-check.mjs`. Preserve click-only, reduced-motion, focus-return, and the five `home-assistant-mark.test.mjs` cases. Goal 1 repaired the four remaining combined-candidate source-gate failures on dirty HEAD `9a922faa` without cherry-pick. Vendor-ui: Marketplace/System `model-management` already matched `_shared`; the check still required retired Home-Agent `model-selection.js` (stale verification; `scripts/vendor-ui-tokens.sh`; `./scripts/vendor-ui-tokens.sh --check` → `[vendor-ui] OK`). Release metadata: packaging already omits home-agent; the worker fixture listed it with size 0, then mock rustc omitted `rustc -vV` host and mock cargo still required `--target` after native Darwin prepare dropped `--target` (stale fixture and harness; `scripts/prepare-release-platform-test.py`; 10 tests OK). Carrier: expected `elastos-identity` 0.6.0 against workspace 0.7.0 (stale verification; `scripts/carrier-dependency-generation-check.mjs`; `ok: true`). Clippy `needless_return`: real defect in `setup.rs` `ensure_bundle_executable_link` unix tail, plus MA4 test-double tails in untracked `gateway_home_system_ai_provider.rs`; `cargo clippy -p elastos-server --all-targets -- -D clippy::needless_return` exit 0. `git diff --check` and `cargo fmt --all -- --check` passed. Alignment and entropy skipped because those surfaces stayed untouched. Receipts `.audit/ma5-pr69-pr68-review-receipt.json` and `.audit/ma5-goal1-source-gate-receipt.json`. Entropy pass on the MA1–MA4 dirty set plus the four repaired gate scripts removed no product code. One-form OpenRouter/Venice UI is already gone. The Jev unavailable writer stays for no named instance, self-evaluation, and an unbound request id. Kept demonstrated callers: Home-Agent route and message compatibility, `openai_compatible_text` offer alias, inline `api_key` migration into Runtime secret storage, and delete/share when exactly one hosted instance of that provider exists. Marketplace and System `model-management` match `_shared`. Linux-preview wording is already gone from the installer docs. `/.audit/` is now gitignored. Receipt `.audit/ma5-entropy-receipt.json`. The existing `-D warnings` clippy log still fails on in-scope style lints in `gateway_model_service.rs`, `model_provider_config.rs`, and `preparation.rs`, plus unrelated Browser, Carrier, and auth lints. `needless_return` is already repaired. Full `just verify` waits. The dirty tree can be split into local commits and that split still waits for an explicit request. |

CR3 stays open before any relay-support claim. Broader R14, J1–J5, SA1–SA6, CA1, Browser and remaining security retain their gates.

The working closeout branch is
`feat/0.7.1-model-assistant-closeout`
`7c65a7cef309eded5def73ed528cbfcdb9b608e4` tree
`66df724f8fc1bcdfacf502c1f92e71e3785bca50`. It starts from published
`fix/0.7.1-security` `54355973e737f018e7d898a74449f9b04aaef26c` tree
`b6442321b77f02382b821f27dc11f397559b2be6` and must merge with or after
that parent. `feat/remote-services`
`8da670b5c3f8a4409a7ac5cb0d14f413ee3b105e` remains an ancestor. Commit
`d1625aaf` amends unpublished `ddfa50f3` so prepare seals an owned Kubo
repo root to exactly `0700`, including umask `0775`, `0755` and `0750`.
`6144fb2a` reports hosted selection facts in Assistant. `ee8b8cd8`
records a Jev Approval Lens skeleton on hosted Assistant
`runs_create`: it writes `recommendation=unavailable`, `risk=unknown`,
`confidence=0`, and `needs_human_review=true`, and it does not call Jev
or show Inbox.
`d8cd6a05` records MA1-MA4 installed evidence. `7c65a7ce` restores
Assistant user prompt text from `modelText`. The earlier isolated Linux
`0775` Get remains cause evidence. The installed sealed ipfs-provider
SHA-256 is still
`8af761a111fdb8b962437e4d530da060569e19defe64c420a26f4e8005227731` and
covers that `0775` path. Darwin candidate Runtime SHA-256
`a908e7b7c1d67b5107eba21f12847d4efecc1ee96601211af37d2ec80ab9f32c`
and Linux candidate Runtime SHA-256
`07a8477cb7582c9d9e3099000716cc5e2fa3115353d94f6c7c6911960639f5a9`
come from this branch. This closeout branch is local only.
Public `release-head.json` reports 0.7.1, release CID
`QmT16KDvZqA4wQc74ssFgy8JJN578Z64AAAhzYvkoE4NF8`, signed by live
`did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj`. Platforms in
that head are `aarch64-darwin` Runtime SHA-256
`82593dfc71f53cffa2dd5fd8c68cbaf4a048890929bf08a85aeb83354558c29f` and
`x86_64-linux` Runtime SHA-256
`63b3cdb06a45523f008fc4e67b50eb795a074890b0a3e839cba2ffa5d532d830`. The
0.1.2 head `QmVLFNQfW6V2LuXCX5xAq1jUmQrReE294Fb2NvETWgNbRk` stays rollback
only. A fresh isolated Apple silicon Home installed from
`https://elastos.elacitylabs.com/install.sh` with no `source add`, listed
verified SmolLM2, completed Get of 144835448 bytes, returned two Assistant
replies, then restored the same chat and model after Home reload and a full
Runtime restart with unchanged weights mtime. Public Home
https://elastos.elacitylabs.com/home/ runs that 0.7.1 Linux Runtime, keeps
the existing account, and lists verified SmolLM2. Earlier public Marketplace
Get attempts failed at MetadataRead while ipfs-provider was the published
`17128b5e` binary. The current public installation uses sealed provider
`0609b6e0`; its public Get awaits a new approved check. An isolated
public-style Linux Home with the
sealed ipfs-provider admitted SmolLM2 through ordinary Marketplace Get,
activated the CPU llama.cpp engine, returned streamed Assistant text
`Pong`, settled Stop as `settlement_unknown`, then restored the same
offer and a second streamed `Pong` after a full Runtime restart. Weights
SHA-256
`c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a`
at 144811072 bytes kept the same mtime. Restart restored a complete
local object replica. Reuse Get completed in under one second with
Bitswap payload received 0. Catalog `content.status` for the admitted
SmolLM2 operation returns `admitted` and `dispatch_ready`. A cid-only
status poll returns HTTP 409 `preparation_unavailable`. That 409 is
harness input, not product UI hide. Assistant `runs_get` without
`request_id` is also harness; live Assistant issues a fresh
`request_id` per run op. The ignored production prepare fixture
`staged_directory_hash_matches_normal_cli_import_without_mutation`
passed with live Kubo and `repo_mode` 448 (`0700`). A throwaway
publisher/consumer with a temporary signed tiny catalogue entry and no
holder completed ordinary Marketplace Get in 0.386 s as
`failed`/`metadata_read` with `completed_bytes` 0 and a 30 KB repo.
The admitted SmolLM2 Home cannot enroll a first-owner passkey because
`require_unowned` sees existing audit ids. Do not wipe that audit. A
fresh isolated Linux Home enrolled an admin passkey through the
virtual-auth product path and created principal-root protection from
that enrollment. Isolated Linux Home `127.0.0.1:61942` completed ordinary
Marketplace Get of SmolLM2 CID
`bafybeidy5kfvqwg6g6pfgdfwslmhijosbeskt5b2duqdqxnc7e6fwmr72y` at
144835448 bytes, then Open in Assistant after a real llama.cpp engine
bundle at `libexec/llama.cpp/b10516/linux-amd64` with mode `0500`.
Assistant returned a Ping reply on offer
`model:01134137a7b2f7e75fe21e610c10bf839a3326770518df2f6d6b29d5216766aa`
as run `run:sha256:c6d09e019c40ce00c248759ce638c34e29a5466829d66768e2d695644f175832`.
Home reload and a full Runtime restart restored that Ping, model
`smollm2-135m-instruct-q8-0-local`, and composer draft. Stop of a later
run `run:sha256:b7aace10d6d00498755177be293dc57357812aa70d413d5e7592649085e338c3`
settled `settlement_unknown` after `runs_cancel` HTTP 200. Public Home
stayed unchanged. Isolated Mac holder and isolated Linux consumer now
load the same `elastos.collaboration-network.startup-config/v1` file
from `elastos collaboration-config`. File keys are only
`schema`, `expected_network_id`, `trusted_profile_signer_dids`,
`profile_chain_base64` and `default_conversation_grant_base64`. The
file contains no model offer. People Profiles connected both ways.
Linux Services projected `MA2 Mac guest's AI model`, sent Ask to use,
and received owner approval. The first remote Qwen reply completed on
offer `model:00c7b9dd517d19449e660149e57f8ec37b5fb7d424405f176fe3afa670f3e3c4`
as run `run:sha256:7bf00494cee50905ef135ccf3708893222c1c71885b0cd693691c23de9e4bbd7`
with request `7b9429c1-c02b-4991-9a16-0acaabff6157` and grant
`services-remote-model-grant-a21916fd3fd3241b`. Transport was
`carrier-provider-plane`. Home reload restored the same Ping
conversation, model and composer draft with zero `runs_create`.
Installed Mac Inbox capsules omit the `service-approve-request`
button. Approval used the Inbox actions API from the Inbox frame.
Remote Stop settled `settlement_unknown` as run
`run:sha256:0d5a802737aba8e43f8ad028cddafa9c9470993451ab2dbabfdaabc766409592`
request `3aa4f7b5-0c75-4815-9411-637cebaa9a2c`. Qwen capsule identity is
CID `bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi`,
content digest
`sha256:93a66d041debbef5daad88c5673666e38187ee64ec136d51b526ab34cdcce212`,
and weights SHA-256
`d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`
at 6169341984 bytes on Mac holder `61680`, Mac catalog `61700`, and
seed catalog SHA-256
`c81d43574da91ad278300eda59330cfdf1fd6a924ef2ebf63658ab0e261d2f2c`.
Seed Kubo holds that catalog file and supplies no Qwen payload. Stopping
llama-server pid `12749` left Home `61680` at HTTP 200 with unchanged
weights. Acquisition on `61942` left no durable llama-server process.
Isolated Linux `61942` now runs candidate musl Runtime SHA-256
`07a8477cb7582c9d9e3099000716cc5e2fa3115353d94f6c7c6911960639f5a9`
with Assistant `agent-stream.js` SHA-256
`515d459139ffb83c44e28a8f683f406ac8d4653ae5d4a06418df97d0b3840575`.
Marketplace still shows SmolLM2 Available. Assistant Settings reports
requested, resolved, provider, limits, privacy, cost and fallback.
Local Ping persist and remote Qwen Ping persist both restored model
`qwen3-5-9b-q4-k-m-local` or
`smollm2-135m-instruct-q8-0-local` with zero extra `runs_create`.
A full `61942` gateway restart then restored model
`qwen3-5-9b-q4-k-m-local`, draft `keep this remote draft`, grant
`503a0102` as `reachable`, and `createsAfterReload` 0.
The renewed grant is
`services-remote-model-grant-503a0102ae5df303` and run
`run:sha256:f5342520a04d9a76591803fc457ce311ecac8d6652118854a8256e8d72d5b073`. Darwin candidate
Runtime SHA-256
`a908e7b7c1d67b5107eba21f12847d4efecc1ee96601211af37d2ec80ab9f32c`
from `7c65a7ce` stays off holder `61680`. Hosted Chat Completions
credential is the remaining operator input for a live hosted route.
Jev Approval Lens on Isolated Darwin 61960 now has installed fixture Inbox
evidence for a named Jev instance. Ordinary hosted HTTP opens Inbox. The
person remains the authority. One request id links recommendation, decision,
and outcome. Live TypeSafe/OpenRouter acceptance stays open. Candidate remote Stop and grant revoke
stay pending. Public deployment of the sealed ipfs-provider needs a
separate approval.
Protected receipts `isolated-qualification-61770.json`,
`isolated-qualification-61780.json`, `isolated-qualification-61800.json`
stay. Full R14, J1–J5, SA1–SA6, CA1, responsive UI, Browser, and remaining
security stay open. Browser stays paused.

The 0.7.1 preview uses the existing public Home at
https://elastos.elacitylabs.com/home/. Ordinary install, Marketplace Get/Use,
Assistant Send/reply, reload and restart remain the demo path. Reuse existing
Qwen evidence within its recorded scope. Publication, public Home cutover and
signed release-head movement stay gated. The executable closeout order is the
stable sequence above.

The broader R14 queue remains required after D1: independent full Mac/seed holder
reads and demonstrated transfer repairs; responsive preparation cancellation and
one total attempt budget; reconstructible cloud input and full Linux consumer
proof; seed-to-Mac Qwen inference; Qwen peer acquisition/pinning without activation.
Reuse unchanged receipts. Browser, J2 updates, three-platform W1 and all J1–J5,
M01–M06 and storage/capsule criteria retain their existing acceptance gates.

The private handoff records the two-hour deadline and target ownership. A failed
demo checkpoint triggers a specific repair or a clear blocker, not a broad audit.

### Preserved Browser closeout (paused during R14)

Notion owns the approved scope and acceptance. This is the executable order;
older mission revisions below are history. Preserve B01-B16, the two accepted
Browser placements, applicable SA1-SA6/CA1, and all existing numerical and human
gates. Current source, target ownership and receipt identities belong in the
private development-loop checkpoint. Existing ordinary persistence, media,
recovery and scroll receipts retain their exact accepted scope.

1. **Reconcile and consolidate.** The Mac leftover file for generation
   `f9cd2b88` is gone after leftover-adapter install `10bf5c6c`. Seed leftover 1
   remains and is the next separate cleanup goal. The isolated Mac Runtime is
   `8849b903` with the B13 Wallet slice and leftover adapter installed.
   Offer `remote-engine-bc829519` stays approved and selectable as
   `remote-engine-4cec4db7`. Shared image `76967c5b` stays unchanged. The
   supervisor overlay is now an explicit `ELASTOS_BROWSER_WEBRTC_SEND_OVERLAY=1`
   opt-in bound to input SHA-256 hashes. Installed diagnostic overlay files are
   removed so ordinary launches stay on the admitted capsule and image. Keep
   the host watcher retired. Keep helper leftover pages `vz-6a0bd4a3`,
   `vz-2d1165cc`, `vz-a35a6fc4`, and `vz-245d4f18`. The five-second audio gate
   stays open. After seed leftover clearance, run one hash-bound supervisor
   overlay capture with a live selected grant checked immediately before launch.
   Local review commit `6aa7c5ae` holds the ICE-pair probe. W1 canary
   `release.json` is signed and still lacks a catalog and components endpoint.
   The reviewed Cloud W1 sequence is local on `feat/remote-services` as
   `2fef812c`, `aac9f263`, and `4009015b`. HEAD is `4009015b` / tree
   `907bbf1f`, 169 ahead of last-fetched `origin/feat/0.7.1-integration`.
   Publication and installer tests pass locally. Cloud receives no local Homes,
   keys, VM images, or unpublished Browser work. Preserve unresolved profiles
   and unrelated dirty work. Attempt chronology lives in the existing
   `u9-r8-*` receipts.
2. **Finish Browser behavior.** After the current media stall owner is named or
   repaired, finish remaining reload, P2 lifecycle, Browser profile
   protection/checkpoint/transfer, authority, selected Engine/Exit, ordinary
   files, and applicable daily-use/accessibility cases. B13 Wallet work is a
   bounded owned slice: pin remaining source gaps with narrow regressions, then
   repair consumer mediation and supported-account consent. The completed
   signature status path now binds page origin and launch on the local
   regression. Local `eth_requestAccounts` honors the selected default
   connector when a managed account is also present, and it keeps origin
   consent. Remote Engine launch now carries
   `elastos.browser.wallet-consumer-mediation/v1` bound to the authenticated
   consumer peer, page and generation. Production forwarding admits a request
   built from that launch payload against the Engine peer plus page and
   generation. Control-service keeps Wallet Bus on the consumer and refuses
   Engine `home_token` fetch for that schema. The remaining gap is Carrier
   transport of the admitted request. Keep Wallet Bus private and keep
   approval on the consumer. Reuse the existing transaction journal.
   Capture the actual ela.city sign-in, mint/buy and playback signing requests
   before compatibility code. Installed local and remote connect/sign-in,
   approve/reject/expiry, wrong principal/origin/page, and response-loss proof
   stay required. Irzhy owns J5 metadata wiring and safe rejected-mint recovery
   at published head `0edd56d5`; that work stays outside this Browser slice.
   Use the shared state contract in
   docs/STORAGE_AND_ACCESS.md and capsule composition in docs/CAPSULE_MODEL.md.
   Browser implements its adapter and only the missing shared mechanisms it
   needs. Builder/Home owns GBA and AI adapters after the contract/evidence
   handoff; those remain required release work outside this Browser task.
   Recheck the complete ordinary local and remote journeys after the affected
   repairs pass.
3. **Prove normal delivery.** Use the common signed capsule/catalog, Content and
   Runtime admission path. Include all Mac Engine helpers, dependencies and image;
   preserve Browser as the entry capsule and separate package identity from
   service offers. W1 supplies an isolated trusted signed candidate before tests.
   This lane holds the reviewed Cloud W1 sequence in three local commits
   `2fef812c`, `aac9f263`, and `4009015b`. Push waits for an explicit ask.
   Prove acquisition from each promised availability source and safe reuse,
   interruption, tamper and compatibility handling. Run fresh Apple-silicon
   installation with empty Runtime data and no borrowed source-home files through
   Home, Browser media/input, reload, close and reopen. Run fresh Linux Home using
   an approved Mac Engine through Services without a local Engine image. Finish
   B15/J2 update and repair with protected profile preservation. Build only changed
   components; bind every installed result to exact artifacts/configuration.
4. **Qualify and hand over.** Freeze the compatible Browser UI, Runtime, Engine,
   image and policy set before the original B11/B16 distributions and endurance
   campaign. Keep manual UX, second-maintainer proof and browser-objective-audit
   required. Changes reopen only dependent evidence and endurance runs. Prepare
   the exact reviewed GitHub/seed candidate, readable results and remaining
   non-Browser release dependencies. Local preparation can precede publication
   approval; push, public deployment and final C5/C6/C7 release retain their
   separate gates. Browser completion requires every applicable Required case,
   not only the last successful experiment.

Cursor is the single implementation and test-lane owner. The current user has
asked to resume and complete Browser. The Cloud W1 task is complete. This lane
holds the reviewed four-file intake in local commits through `4009015b`. Continue between evidence checkpoints;
30 minutes is a progress/replanning boundary, not an automatic stop. Each update
names accepted behavior or a narrowed cause, the next proof and any missing
input. A repeated identical failure needs a changed experiment before another
long run. Review coherent source and installed milestones; reuse unaffected
checks and receipts. When human input or a target is unavailable, state the exact
blocker and continue independent Browser work. Maintain the 10 percent disk floor
and preserve user Homes, keys, drafts, profiles, donor work and public services.

Mission revision R3, 2026-09-12 (reconciled 2026-09-13 UTC). The
[approved five-journey plan](https://app.notion.com/p/wauio/ElastOS-0-7-1-release-plan-five-user-journeys-from-start-to-stop-3d6b682adcca81948f78d12abcd677b9)
owns J1-J5 acceptance and D1-D6 decisions, including the remote inference
amendment. This section owns execution order; [state.md](state.md) owns source,
installed and public evidence. Earlier delivery dates and allocations are history.

`feat/remote-services` is the active candidate, extending the published PR64
checkpoint with the Browser donor and remote Qwen work. Keep its reviewed history
and the separate dirty updater slice. Public preview remains on its accepted
September 11 artifacts. The [team report](docs/audits/2026-09-11-team-sync.md)
and [contributor review](docs/audits/2026-09-11-contributor-review.md) retain their
5-11 September scope; current progress is recorded in state.

Use one execution owner for both installed test Homes. Resume the existing
Browser owner for the next bounded slice; keep Model target use behind an explicit
handoff. A reviewer checks the result at the evidence boundary. The existing hourly
monitor reuses recent reviews and inspects new evidence. Prepare the candidate
locally for review; publication and public cutover retain separate approvals.

Historical Browser slice, 12 September 2026: close the then-live 110 page
through Home, then prove actual viewer reload within five seconds,
input/media and all 13 close effects on Engine
`remote-engine-ec54fd5eab67a5c5913513a47a797fc8` with seed Exit
`source-home-browser-exit`. That grant and page are no longer the current
U9 target. Ordinary Automatic open still fails at profile placement when a
valid grant exists. Select the owning Mac Engine through Settings.

The independent Qwen slice is one immediate Busy replay and retry after capacity
frees. Review the prepared helper only when that slice starts. Preserve accepted
reply, replay and revoke evidence. Complete restart during an active run and
second-principal denial, then join accepted U8/U9 in U10. Marketplace handoff,
safe removal and cold Content/Carrier delivery remain required under J3; AI1-AI6
in the approved plan separate that work from later provider and business stages.

The C1-C7 sequence below retains the full release gate.

| ID | Outcome | Status / owner | Required input and next proof |
| --- | --- | --- | --- |
| C1-site | A useful, truthful public storefront | Public preview deployed / coordinator | New storefront and canonical Home link pass public HTTPS hash and browser checks at `25966622`. Confirm team review. Installer control stays disabled until signed 0.7.1 delivery is served and verified. |
| C1-home | Human Home entry at `/home/` | Public route verified / coordinator | Public `/home/` and legacy redirect pass; the new sign-in screen renders without console errors. Anders confirmed existing-account sign-in and saved work. Full J1 sign-out/shortcut acceptance remains open. |
| C1-proof | The execution loop handles a real reviewed slice | Accepted for source review and monitoring / coordinator | Independent source/rendered review, nine process cases and actual recurring monitor delivery pass. Continue through installed and public proof under C1-seed. |
| C1-seed | Seed serves the reviewed local candidate and storefront | Bounded deployment accepted / coordinator | Authorized `25966622` deployment passes integrity, 573 artifact comparisons, running/served hashes and zero-migration guards. Account/user-file hashes are preserved. Anders confirmed existing-account sign-in and saved work; the temporary stage and deployment rollback are removed. Full journeys, Browser/protected-video target configuration and signed delivery retain their gates. |
| C1-delivery | Signed three-platform candidate installation | Functional preparation complete; final assembly after C5 freeze / delivery owner | Native inputs, bootstrap and publisher import have bounded source/artifact proof. Keep working builds for functional tests and rebuild only affected components. Complete atomic promotion, final installer stamping, platform regeneration and signed fresh installs at the final release boundary. |
| J1 / C2 | Install, passkey, Recovery Kit, Profile and usable Home | Human passkey proof pending / Home and installer owners | Registration binding and atomic/conflict-safe identity persistence are integrated with 66 focused tests passing. Durable owner intent and Profile-inclusive recovery now pass source and UI checks. Real signed Mac installation and seed export pass. Source repairs for kit-first recovery, gateway shutdown and media-tool reuse are accepted. Both targets pass installed browser recovery/reload/retry, complete owned shutdown/restart, full Home setup/media reuse and Carrier cleanup/reuse. User-review account label, Recovery/Advanced spacing and cross-port session isolation are repaired on `1e320578`; shared-browser Mac/Linux proof passes. Next: AUTH-01 final user acceptance on the updated preserved Homes; concurrent-save and window/selection UI; W2-W5; rerun the smoke with System entry through the ElastOS menu, then complete the app matrix; full Linux/Mac staging. Publish J1 for review first under D1. |
| J2 | Ordinary update preserves user state | Planned / delivery owner and independent reviewer | Coherent update commits on J1; Mac platform selection is repaired in 52fc231e with 11 updater tests; establish the actual stamped 0.7.0 starting updater. The published tag and local 0.7.0 tag differ; a source-home replacement cannot close first-hop proof. Candidate/interruption acceptance remains C6. |
| J3 / C3 | Obtain a model through Content/Carrier and use it locally | Paused after checkpoint publication / coordinator; independent review | The local-model checkpoint is preserved in draft PR64. Resume its remaining work from the active candidate named in Now. Draft PR64 contains the complete combined checkpoint; the human Mac Home now has one Assistant and admitted Qwen. Bootstrap is accepted. The full corrected donor `6972e165` is merged by `b318bfda`, and Sash shelf ancestry is reconciled by `37c82d4f`. Irzhy foundation `617796a9` is merged by `dd21d8bd`; further intake is frozen for one installed Qwen journey. Mac catalog, Use/cancel/retry, exact admission, cold startup, actual reply and saved conversation pass on installed Runtime `fbf1a4b0`. Reply/draft/exact selection survive reload without implicit dispatch. Stop shows honest unknown, as allowed by Step 5; confirmed cancellation remains unproven. Owned shutdown, restart and second reply pass with identical package files, restored draft and zero received Bitswap payload. Installed model-menu empty-state/placement and workspace-preservation checks pass on `8f28b6e3`. The source-install metadata helper repair and its regression checks pass. The adapted URUX/UIUX candidate already entered through Irzhy reconstruction; the full old-tip merge recommendation is superseded. The approved one-Assistant implementation now preserves Sash’s UI in the canonical capsule, adopts all three stores into protected v2 storage, retains complete history/drafts, and preserves concurrent edits and run ownership. Source and rendered checks pass. Candidate `fa297cb5` now also passes installed one-Assistant catalog/legacy launch, real Qwen reply/save/reload and full Runtime restart with the original draft and exact model intact. The human Mac Home now has an enrolled passkey account and a verified locally cached Qwen package. Its System/Marketplace launch metadata defect is repaired by the `822c4e3d` installer guard and corrected receipts; both apps open in the actual browser. Anders confirms Assistant works. Current `3c2f9a80` repairs pending Keep intent, normal Marketplace Models category/details, theme tokens and duplicate activity/headings. Source/rendered checks and installed Mac retention readback, Qwen reply/save/reload and human model-view observation pass, with matching receipts and user data preserved. Pending preparation itself uses source/rendered proof; the installed model was reused. Linux still needs disk headroom and a reviewed startup profile. Next required work is exact Marketplace-to-Assistant handoff, safe removal and cold Carrier delivery. Broader execution and monitoring pause at this user-feedback checkpoint; full human J3 remains required. See [the convergence check](docs/audits/2026-09-11-assistant-convergence.md). URUX and Irzhy authority-cutover → follow-up work retain their Required gates and dependency order. Track every adapted or pending feature in [the preservation check](docs/audits/2026-09-11-integration-preservation.md). Preserve current recovery, sessions, media reuse and shutdown. The full merge includes storage/window source; retain their separate installed J1 acceptance. Prove normal controls and one bounded test-owned Home preparation attempt before large transfers. Complete acceptance steps 1, 2, 3 and 5 in section 6; hosted/Codex remains Optional; remote inference is Required under C3R/M01-M06, and Jetson remains Later. |
| C3R / U8 | Approved seed user runs the Mac model | Partial installed acceptance / next execution owner | Accepted results are in state. Next: immediate Busy replay and successful retry, restart during a run without another dispatch, and denial of an unapproved second principal. AI1 adds exact package/offer/terms identity and Marketplace handoff before final M01-M06 acceptance. |
| J4 / C4 / U9 | Browser works in the two agreed placements | Functional proof open / next execution owner | Ordinary A/B/A open 110 completed after Engine grant renew and Mac Engine selection. Close that live page, then prove actual reload within five seconds, audio/video/input and all 13 close effects. Then finish authority, recovery, state and daily-use checks; B11 performance and B16 qualification retain their gates. |
| U10 | Qwen and Browser work together | Waiting for U8 and U9 / execution owner | One seed principal uses both on the same Mac; each keeps its own acceptance criteria. Include AI1 when its UX/identity changes enter the candidate. |
| J5 | Protect, list, buy, play and close controlled video | Implementation open / protected-content owner | Irzhy published head `0edd56d5` on `feat/protected-content-0.7.1-followup` adds connector mint, one wallet effect, terminal approval failures and completed-mint replay. Metadata URI/royalty/Creator wiring and safe rejected-mint recovery after `EffectRaised` remain his J5 work. Browser now has a local completed-signature origin/launch bind regression, local `eth_requestAccounts` honors the selected connector when a managed account is also present, and remote Engine launch mediation plus production admit bind authenticated peer, page and generation. The remaining B13 Wallet gap is Carrier transport of that admitted request. J5 stays one Runtime, two principals, Brave and Base mainnet; funded runs still need the agreed spend limit. Installed/human acceptance remains Required. |
| C5 | Assemble and freeze the reviewed candidate | Planned / coordinator | Reviewed required source; exact optional task list. Optional work cannot delay freeze. |
| C6 | Accept the combined signed candidate | Planned / independent reviewer and human testers | All required journeys, shared Home/app regression, Browser qualification and first-hop update proof on matching artifacts. |
| C7 | Publish and verify the accepted release | Planned / release owner | C6 and explicit approvals, accepted main tree/artifact identity, then public delivery checks. |

User sequencing clarification: finish functional journey work before final release
assembly. Preserve evidence for unchanged components across source updates. Build
affected components for testing as needed; stamp the final installer and produce
the final signed platform packages after the source freeze.

Immediate preview delivery: the fixed d790 candidate passed Mac installation
and seed export; the user also installed it. The observed shutdown,
duplicate-download and kit-first recovery repairs now pass source checks.
The combined automated installed checks pass. AUTH-01 needs a user-operated
passkey on the prepared matching artifacts; preserve its pending verdict. Preserve all user test Homes. The public preview is deployed and Anders confirmed existing-account
sign-in and saved work. Jetson is deferred
at the user's request. Wider J2–J5 work and final three-platform assembly remain
required for release; unchanged artifacts retain their evidence.

The required shared regression includes Desktop/Terminal and Inbox handoff,
Create/Recover, Recovery Kit coverage, Profile and window placement, document
save conflicts and picker acknowledgements, drafts, games, Archive/Library,
Chat and repeated System Save. One combined record serves J1 and J3. The
[journey register](docs/audits/ElastOS-Home-Journey-Audit.xlsx) retains the
underlying findings and their proof requirements.

Existing protected-content two-Runtime, independent-operator and wider Browser
goals keep their Later gates as set by D2/D4 and the approved plan. The 15 September
J5 scope decision uses Base mainnet for bounded acceptance, with dedicated test
wallets/assets and an agreed spending limit before funded runs. Controlled failure
and replay tests remain local; Anvil repair is outside the critical path. Preserve the
protected-content Runtime authority cutover, DocumentsSaveRequest/if_revision,
bridge lifecycle, completed-mint adoption and cleanup obligations during
integration. Required failures stay open until the relevant proof passes.

- [ ] Turn the journey audit register into an automated pre-release gate:
  extract complete-verdict Journey Matrix rows into checks using the existing
  smoke-script pattern. Installed and human journeys keep their own authoritative
  register verdicts; source automation covers only the behavior it can observe.
- [ ] Review the PR15 follow-up for the canonical `has_access_by_content_id`
  selector (`0x54d42821`), whose configuration currently validates shape only.
  Use a gated channel for deny proof; a permissive token threshold remains a
  separate operator choice. Carry this review with J5/C6 acceptance.

## Later

### Protected-content foundation intake

- [ ] Verify the merged `617796a9` provider-host, Chain, Content availability and
  Carrier peer-seeding source with current model lifecycle and endpoint cleanup.
  The donor's simulation-only three-node and Anvil fork proof remains historical
  evidence, now retained in state.md and deploy/custody-host/README.md.
- [ ] Before installed custody-host acceptance, an explicit Carrier listener
  either binds the requested address or reports failure; it must retain the
  configured relay policy. Preserve the default-address compatibility behavior.
- [ ] Follow with authority cutover `25ab205e`, then ready follow-up `06179578`
  work. Distinct seed/third-node hardware, independent operators, funded real
  Base and human Brave acceptance retain their existing J5 gates.

### Retained source integration

The August 31 source-merge scope leaves these items open. Their donor work is
preserved; an older implementation is not evidence that it fits current contracts.

- [ ] Adapt Carrier's incoming request size and deadline limits from the
  retained Carrier work without changing current provider-invocation semantics.
  Cover oversized, slow and incomplete frames plus valid protected-content
  calls. Review the broader signed-protocol and peer-budget work separately.
- [ ] Review the retained native-component interoperability design as a
  separate proposal. COMO adoption needs its own feasibility and isolation proof.
- [ ] Adapt advanced Assistant workflows under
  [Sash UIUX integration](#sash-uiux-integration).
- [ ] Resolve legacy-auth migration under
  [Operator and audit hardening](#operator-and-audit-hardening), preserving
  historical audit evidence and existing identity state.

### Content distribution and Windows

- [ ] Implement the signed content-capsule catalog and typed Get contract in
  [docs/CONTENT_CAPSULE_DISTRIBUTION.md](docs/CONTENT_CAPSULE_DISTRIBUTION.md).
  Use the complete bundle CID, publisher verification, availability evidence,
  atomic admission, install/removal receipts and partial-download cleanup.
  Keep large model bytes outside Git and external repositories behind providers.
- [ ] Prove the WSL-first strategy in [docs/WINDOWS.md](docs/WINDOWS.md) on a
  fresh Windows machine, including stable localhost/passkey origin, Recovery
  Kit, Profile, Wallet, People, Chat, restart, update and cleanup. Native Windows
  remains later adapter work; preserve the known Unix portability gaps.

### Consequence-aware effects

- [ ] Before shipping a physical or retry-sensitive provider, define its typed
  observation or actuation contract and provider operation classification.
  Runtime must enforce or strengthen that classification. Also define the
  effect-ID and reconciliation policy, deadlines, settlement evidence, and
  local safety owner. Prove that capsule metadata cannot downgrade the policy,
  remote authority cannot bypass a destination interlock, and reconnect or
  timeout cannot repeat an uncertain effect. Keep field buses, controller
  protocols, peers, host paths, backends, and credentials below the provider
  boundary. Do not add a separate OT authority stack or claim Runtime is a hard
  real-time controller.

### Collaboration identity and Carrier boundary

- [ ] Define and prove multi-device Profile pairing after installed
  collaboration acceptance.
- [ ] Add direct-message attachments through the typed Runtime contract after
  installed text-message acceptance.
- [ ] Define the wider-rollout rendezvous and abuse-control plan for People
  discovery without weakening the current source guarantees: discovery stays
  opt-in and bounded to `Visible now`, seeds remain configuration/rendezvous
  authority only, isolated and alternate networks stay possible, accepted
  contacts continue without the seed online, and today's fixed-cap relay proves
  bounds rather than a global roster or 100k-user scale.
- [ ] Treat the provisioned shared conversation as an explicit onboarding room,
  not the architecture for all group Chat. Direct conversations are between
  accepted Profile identities. User-created groups need their own stable
  conversation identity, signed membership/grants, bounded discovery, and
  durable catch-up. Joining one collaboration network must never enumerate or
  subscribe a person to every user or every group on that network.

#### Offline delivery

- [ ] Implement encrypted mailbox delivery per the boundary recorded under
  "Later work" in
  [docs/COLLABORATION_HANDOFF.md](docs/COLLABORATION_HANDOFF.md), after
  the installed two-runtime acceptance. The design fixes the holder model (any
  Runtime via Services
  offers, seed never special), sealed-envelope deposits with receipts
  travelling symmetrically, unchanged delivery truth and end-of-life, the
  named holder metadata, holder-enforced retention, and the
  `collaboration-mailbox` provider running on `collaboration_delivery.rs`.
- [ ] Implement app-declared data retention when a second consumer exists.
  Designed: the declared-policy pattern Chat already ships
  (`DECLARED_DIRECT_HISTORY_POLICY`) generalizes to a per-app, per-object-class
  declaration the Runtime enforces at the read model — `SurvivesRelationship`
  (direct history today), `GatedByRelationship` (the contacts-derived refusal),
  and `RevocableEncrypted` (the future `elastos://vault` class). Refusing a
  read stays a capability an app declares, never an emergent side effect.
  Chat is the only consumer today, so the generalization waits for the second
  one rather than abstracting from a single case.
- [ ] Implement silent block after installed acceptance. Decided: block is a
  separate verb from remove, not a variant. Remove stays the honest signed
  default that tells the other side. Block is local-only: no wire artifact is
  minted (silence is the feature), a local flag on the relationship record
  stops both delivery directions after verification, no receipt is returned so
  the blocked sender sees ordinary non-delivery indistinguishable from
  offline, and the People state machine reserves `blocked` beside the removed
  states. Unblock clears the flag locally with nothing to re-announce.

#### Sash UIUX integration

- [ ] Add a People-owned ambient indication while opt-in discovery is active,
  without giving Home read access to People's private discovery state.
- [ ] Define a typed, attributed capsule-rendered compact panel surface only if
  the product needs app-owned controls inside system chrome. This is future
  surface work, not a migration shortcut.
- [ ] Restore the intended Sash Assistant workflows with explicit preservation of existing features and workspaces. Establish typed Runtime contracts and
  accepted product scope exist for them: desktop attachment, knowledge/citation
  and search flows, rich media preview/open, and advanced Studio
  inputs/workflows.
  Retain the August harness and Studio donors as implementation evidence:
  `origin/feat/home-agent-harness`, `origin/feat/home-agent-harness-rebuild` and
  `origin/experiment/home-studio-h3-dogfood`. Their retained source is separate
  from inclusion in the standalone Assistant. Port the useful behavior through
  typed resources; preserve the existing offer, run and workspace contracts.

#### First run on a clean Home

- [ ] Complete the clean macOS Browser installation path. Source-home setup
  already installs provider/helper components and generates Browser config;
  a fresh target still needs the matching VM substrate and installed product
  proof. Define their package/profile ownership and use
  `browser-vm-engine-preflight.sh` to identify missing artifacts before launch.
- [ ] Scope the Browser VM control socket and root to the Home. The adapter
  config generator derives both from the platform alone —
  `/tmp/elastos-browser-vm-control-darwin-arm64.sock` and `/tmp/evzs` — so
  every Home on one machine writes the same two paths, and a second Home
  either attaches to the first Home's VM control plane or clobbers it.
  Anyone running a test Home beside a real one hits this, and it presents as
  Browser breaking for no reason. Derive both from the data dir.
- [ ] Re-read the first-run copy once the above lands. "Create your
  Profile / Create a Profile to use People." says the same thing twice and
  explains nothing about what a Profile is for. The product review below
  should own this, but it needs a Home where the journey completes.

#### Additional product proof

- [ ] Run the app-window matrix somewhere. `just product-ui-virtual-auth` is
  now the explicit operator command, but it has not yet run on an installed
  Home. The completed source-side receipt is in place: the shell now records
  `load` versus `timeout` reveal cause, the portable Home regression smoke
  asserts `load`, and the real app-window matrix checks effective ancestor
  visibility plus occlusion instead of only the iframe's own style.
- [ ] Delete the plaintext left behind by protecting a root. A successful
  migration keeps its owner-only backup of the original unencrypted bytes
  under the Home's backups directory forever. That is a reviewable artifact
  on the offline upgrade an operator runs deliberately; on the automatic
  path it means every Home that protects a root ships an unencrypted twin
  of what it just encrypted, with nothing telling anyone it is there.
- [ ] Roll back, or refuse earlier, when protecting a root half-applies.
  Protection is stored before the objects under it can be encrypted, so a
  migration that fails afterwards leaves a protected root with plaintext
  beside it, which the next boot refuses to start on. The count that made
  this reachable is now checked before the first write, but an I/O failure
  mid-migration still lands there, and the online path — unlike the offline
  one — neither rolls back nor treats a recovered journal as fatal.
- [ ] Compare the whole launch token, not just its context, when a header
  and cookie disagree. Both must verify independently and the cookie wins,
  which is the right direction, but equality covers principal, session,
  proof binding and grant only — so two tokens for the same session with
  different launch contexts compare equal and the cookie's launch id
  silently wins the audit trail.

#### Later collaboration follow-up, blocks no release gate above
- [ ] Review the unchanged power-user `elastos chat` and remote `room` CLI
  paths from released `main`. The packaged Chat and Agent capsules and their
  direct Carrier callers are removed in this candidate, but these operator CLI
  commands remain. Either bind them to the same typed Runtime collaboration
  resources or retire them in a separate change; do not add another Chat state
  or transport path during candidate closeout.
- [ ] Decide what presence means, now that a Home can announce without a
  person present. Announcing from the Runtime obeys the Discovery opt-in,
  so nobody is broadcast who did not ask to be — but the meaning of the
  signal changed underneath the switch: it used to say "someone is using
  this Home" and now says "this Home is running", and it reaches everyone
  on the collaboration network rather than only accepted contacts. Decide
  whether a person wants contacts to see a machine that is merely up, and
  whether presence belongs to contacts rather than the whole network. The
  answer likely splits one signal into two.
- [ ] Reach the Telegram bar for offline people, in both direct and group
  conversations. A message sent to someone who is not there should arrive
  when they return, and a member who was away should come back to what the
  group said. Neither holds today: a direct envelope ends terminal and
  visible after a 24-hour lifetime, so a longer absence loses it rather
  than delaying it, and group messages ride a gossip topic buffer that
  lives only in memory per peer and is discarded on restart, so what a
  member missed survives only while some peer happens to still hold it.
  The shape that closes it is a Runtime that is always there — a person's
  own Home where it is always on, and a backup Runtime holding for them
  where it is not. That is the accepted encrypted mailbox
  (the "Later work" section in `COLLABORATION_HANDOFF.md`), which must stay a holder of sealed
  envelopes it cannot read rather than becoming a special server. For group
  catch-up, decide between carrying it in the mailbox too and giving each
  member a durable append-only signed log a returning peer can request
  ranges from, the model Hypercore and Pears use, which fits the signed
  objects this Runtime already has and answers "what did I miss" after a
  restart, which the buffer cannot.
#### Housekeeping, blocks nothing above
- [ ] Prove the old `codex/0.6-release-hardening`,
  `feat/collaboration-network-profile`, and
  `fix/collaboration-chat-session-bootstrap` lines are contained by patch or
  ancestry, then remove their clean worktrees and local branches.
- [ ] Inventory the remaining historical branches, archive refs, worktrees, and
  stale remote-tracking namespaces. Bundle or retain unique evidence; remove only
  clean, proven duplicates. Do not mix historical archive work into the active
  product branch.

### Established product follow-up boundary

- [ ] Keep Browser included but explicitly limited: address intermittent
  restart, non-retained `ela.city` login, and slow performance before claiming
  full Browser reliability. Preserve exact-once Wallet approval and
  Runtime-only networking while fixing these issues.
- [ ] Decide the policy for plaintext principal roots. The current
  hidden upgrade migrates only roots that already have protection metadata; it
  is not a general 0.5-to-0.6 data migration. Either provide an explicit,
  user-approved reset for unprotected roots or design a separately reviewed
  migration, then remove compatibility machinery that has no supported user
  journey.
- [ ] Replace administrative Browser cleanup retirement with provider-owned
  durable child/process identities and idempotent terminal cleanup receipts.
  Restart recovery must settle exact obligations from explicit retained
  identity, not process-list, port-availability, or socket-inactivity inference.
- [ ] Preserve the remaining `feat/shell-ui-esp-on-protocol` donor work until
  its deferred Assistant scope is reviewed. The reviewed shell and UIUX work is
  included in `main` and `origin/upstream/0.7.1-dev`; any further extraction needs a source comparison against
  the current ESP contracts.
- [ ] Resume Carrier reconciliation only after its provider generation,
  multi-node physical evidence, cleanup, and release boundaries are reviewed.

### 0. Branch readiness and reviewability

Branch assumptions: `origin/main@8ac18bec` contains the released `v0.7.0`
source. `origin/upstream/0.7.1-dev@c511b133` is the active integration line.
PR52 (`4d688cc5`) and PR54 (`2a49ea57`) are active review branches. Use
[state.md](state.md) and fetched refs for exact checkpoints. Published source,
installed behavior and public-live behavior require separate evidence.

- [ ] Keep this branch reviewable: split changes into coherent commit slices with no corrective commits, no hidden migrations, and no unrelated local artifacts.
- [ ] Keep oversized-file cleanup frozen unless branch review exposes a concrete no-behavior blocker. The existing Browser/Wallet/provider cleanup is already split into focused sibling modules: Browser gateway, Wallet gateway, Wallet UI send/receive/create/request/state/preference flows, wallet-provider EVM crypto, and wallet-provider approval test groups. Keep those seams stable and verified. Do not split `capsules/browser/browser/browser.js` further unless a diagnostic-frame/session seam is proven mechanical and behavior-free. Treat `gateway_tests/room.rs`, `gateway_room.rs`, `gateway_tests/home_system.rs`, `room_service.rs`, `auth_gateway.rs`, and `home_cmd.rs` as later cleanup unless they become direct release-review blockers. Keep `scripts/home-entropy-check.mjs` as a broad alignment gate for now, but do not let it accumulate new product logic. Each future split must be no-behavior, separately testable, and covered by the narrow Rust/JS smoke commands for that surface.
- [ ] Review this branch in authority-bound slices, not as one Browser mega-diff: content availability/protected content providers, chain provider core, auth/recovery core, Wallet authority surface, Home/System UX, Chat/Carrier updates, capsule authority manifests, Browser ABI/adapter, Browser proof tooling, shared runtime/gateway, then release/registry/docs. Each slice must be a coherent commit with its own verification commands. Shared runtime/gateway hunks require manual hunk-level review because they cross provider boundaries. Keep `chain_provider_core` separate from Browser: typed proof, prepare, broadcast, sync health, and node lifecycle are blockchain-quadrant provider work, even when Browser consumes them through Wallet. Keep `auth_recovery_core` separate from route wiring: passkey/WebAuthn verification, proof-bound sessions, principal roots, and Recovery Kit helpers are authority primitives consumed by Home/System/Wallet gateway routes. The Wallet authority surface should be reviewed as provider authority core, Wallet app and connector capsules, then gateway/Inbox/audit wiring only after shared gateway hunks are isolated. For Home/System UX, run `node scripts/home-passkey-virtual-auth-smoke.mjs` on loopback Home to prove signed passkey journeys without a human cookie, then run the Camofox smokes for layout coverage. For Browser ABI/provider work, run the Browser Rust tests, `scripts/check-wci-alignment.sh`, `node scripts/home-entropy-check.mjs`, `node scripts/browser-display-mode-smoke.mjs`, `scripts/browser-wallet-bridge-smoke.sh`, and `scripts/browser-glide-wallet-smoke.sh`. Browser proof tooling must keep provider decision reports, objective audits, and runbooks structured and fail closed while product media/manual evidence is missing. Each slice must name its verification commands and must not claim Browser completion unless `scripts/browser-objective-audit.mjs` passes with accepted product media plus matching manual UX evidence.
- [ ] Do not reopen the accepted released-line reconciliation except for a newly
  proven defect with a named owner and verification command. ESP,
  Wallet, Recovery, Home authority, GBA, and the bounded Browser continuation
  are reconciled in [state.md](state.md). The reviewed shell/UIUX is included;
  broader Carrier reconciliation and advanced Assistant workflows remain
  deferred. Before claiming
  completeness, run `git diff --check`, the Home and Browser entropy checks,
  WCI alignment, `just candidate-command-audit`, and touched-surface tests.
- [ ] Treat Remote Carrier Exit as part of the Carrier slice: two-runtime evidence must cite the exact source/exit runtime DIDs and endpoint evidence; the installed artifact readiness report and route-readiness report must be hash-bound; evidence for route readiness, installed artifact readiness, discovery, policy, accounting, stream transport, Browser proof, and cleanup must cite reviewed route nouns; the local Browser machine-proof artifact must cite the reviewed route target or target host; local artifacts must stay redacted, and remote paths need an explicit digest and review trail. Compose Inspector, typed Runtime authority, installed artifact readiness, route-readiness, operator evidence, Browser handoff, manual UX, performance/zoom, and clean-worktree proof before any full-goal claim.
- [ ] Keep the verification gate green after each slice: run Rust workspace commands from `elastos/` such as `cargo fmt --all -- --check`, the narrow Rust tests for touched crates, `cargo check` for changed capsules, `git diff --check`, `scripts/check-wci-alignment.sh`, `scripts/protected-content-provider-contract-smoke.sh` only as the provisional provider retirement guard where those old capsules are touched, `node scripts/home-entropy-check.mjs` where Home UI is touched, `scripts/auth-wallet-focus-smoke.sh` after auth/wallet/chain changes, `scripts/installed-provider-verify.sh <provider>` after installed provider binary changes, and a live `/apps/home/` proof before handing browser-visible changes back for testing.
- [ ] Do not add visible UI, protocol surface, provider behavior, or blockchain hooks unless the runtime capability path, fail-closed behavior, and docs contract are already explicit.
- [ ] Keep first-party capsule projection validation covered by
  `first_party_capsules_have_complete_projection_contract`; extend the same
  Runtime-derived proof whenever a capsule adds web, CLI, fact, affordance,
  gate, audit/mirror, or Carrier/service surfaces.
- [ ] Freeze new Browser provider implementation and other speculative Browser provider work until the current Browser objective blockers are cleared or explicitly rescheduled. `scripts/browser-provider-decision-report.mjs` is the active decision surface: Docker/Selkies is the current hosted proof path, not final product completion. Live Browser must keep one isolated engine/control session per Browser capsule launch/window, keep that stream alive while the Browser window is open, reconnect through Runtime when WebRTC/page heartbeat is lost, and fail closed if page-scoped control is missing; do not reintroduce an always-on shared global hosted browser session, the old serialization blocker, or `hosted_browser_session_busy` user path. Stale Browser launch authority must relaunch through Home/Runtime for a fresh non-delegatable app token; Browser must never refresh or mint its own authority. The first Browser Session Manager foundation now exists in the gateway/adapter path: launch reservations, per-principal/total capacity limits, close-path release, page activity touch, Browser page heartbeat, session-capacity summary receipts, stale-active ledger cleanup with provider-owned `close_page`, adapter `max_active_sessions`, and clear `browser_capacity_unavailable` errors. Finish the remaining product-readiness parts before any Browser product claim: operator capacity/status diagnostics, resource accounting, tab/page ledger support, clear `browser_session_start_failed` receipts, and long-hold/concurrent smokes that assert heartbeat continuity and orphan cleanup. Short open/close smokes are not enough; Browser release evidence must prove no frame starvation, no orphaned launcher/container, and clean shutdown. This server cannot prove native product media without real display/audio/network isolation, and Kasm Workspaces/BrowserBox cannot be accepted until operator prerequisites plus the hosted bake-off and hash-bound manual UX evidence pass. Hosted WebRTC manual evidence must separately record advertised audio, explicit user-gesture unlock, unmuted/remote-audio-enabled status, and received-audio evidence before YouTube audible audio can count; `scripts/browser-manual-ux-report.mjs` requires short evidence text for those hosted audio fields, not just boolean checkmarks. Use the decision report's structured `next_action` field, `scripts/browser-provider-runbook-smoke.sh`, and the artifact-aware `scripts/browser-provider-runbook.mjs --hosted-bakeoff/--native-preflight --manual-ux` handoff as the current machine-readable driver; do not spend more branch time tuning Selkies as the product path.
- [ ] Keep the Browser provider proof language explicit: Selkies is the current self-hosted baseline, not the acceptance answer. Native/browser-product proof must stay tied to `browser-native-supervisor-smoke.sh`, `browser-native-proxy-engine-smoke.sh`, `browser-native-supervisor-proxy-smoke.sh`, `browser-native-operator-config.mjs`, and `browser-native-target-preflight.sh`; Browser wallet connector effects must keep `wallet-connector-transaction-smoke.mjs` in the verification set.
- [ ] Keep protected-content release claims exact: `scripts/browser-ela-city-protected-content-open-smoke.sh` proves that Runtime Browser can open the known `ela.city` protected-content route and cleanly release the page session, and the current branch has a funded live purchase/playback proof for the known test path. Release notes may cite that current user journey, but must not claim arbitrary protected-content readiness, production dDRM completeness, dKMS readiness, or generic decrypt/render provider completion.

### Released-product proof and follow-up
- [ ] Review execution order for maintaining and rechecking product proof against
  released `main`:
  1. Keep reusable source/review gates green on this branch:
     `git diff --check`, `node scripts/home-entropy-check.mjs`,
     `node scripts/browser-entropy-check.mjs`,
     `bash scripts/check-wci-alignment.sh`, and touched-surface Rust/capsule
     tests.
  2. Run source/install command gates that do not require human target action:
     `just candidate-command-audit`, `just verify` when time allows, and the
     relevant Browser/Wallet/Home smokes for changed slices.
     Keep private proof logs outside the public repo; record only public-safe
     proof status and command names here.
  3. With a Home-authorized Browser page open on Jetson, run the strict target
     gate:
     `scripts/jetson-browser-runtime-audit.mjs --host <target-host>
     --user <target-user> --data-dir <target-elastos-data-dir>
     --source-dir <target-source-checkout> --require-parity
     --min-active-crosvm-seconds 3600`.
  4. Run manual installed-device checks on Mac and Jetson: `elastos setup`, open
     Home, visit System, Documents, Library, Inbox, People, and Services, launch
     and close at least one app, then return Home cleanly. Source-home proof does
     not close this item.
  5. Keep source/local Carrier setup proof green with
     `scripts/local-carrier-setup-smoke.sh` before a staged or published
     gateway exists for branch-override install proof. Branch-override public
     install proof with the branch binary needs a staged or published
     release-compatible manifest with the current `home` profile and
     checksummed artifacts; then rerun
     `scripts/public-install-identity-smoke.sh` and
     `scripts/public-install-home-frontdoor-smoke.sh` with
     `ELASTOS_PUBLISHER_GATEWAY=<candidate-url>` and the branch binary override.
     After final publish, rerun both without overrides.
  6. If Browser product readiness is in scope, keep
     `scripts/browser-objective-audit.mjs` red until accepted hosted/native media
     proof and hash-bound manual UX evidence exist; otherwise document Browser as
     architecture/proof-path reconciled, not complete.
  7. Finish with `git diff --check`, an entropy pass over release truth
     surfaces, and changelog/release notes that claim only the proofs above.
- [ ] Cross-host closeout slice: keep reusable source gates, relevant Rust tests, and clean-tree proof green before claiming all Mac, Jetson, and server work is represented. Do not treat docs-only updates above a proof target as full proof until the actual commands have been rerun. Local must remain clean, target source trees must match the reviewed branch, and any target audit must use explicit host/user/data/source arguments rather than committed SSH aliases or local paths. Any newly found host delta must cite the source host role, intended owner slice, and verification command before it changes this branch.
- [ ] Live target closeout slice: keep production/stable target runtimes separate from this review branch. Target evidence must cite the exact reviewed branch, source tree, data dir, and command used without committing private SSH aliases, keys, tunnel ports, or operator paths. Keep `scripts/jetson-browser-runtime-audit.mjs --host <target-host> --user <target-user> --data-dir <target-elastos-data-dir> --source-dir <target-source-checkout> --require-parity` free of parity failures before target-closeout claims; active Browser product proof still requires a Home-authorized Browser open, long-hold evidence, and manual UX evidence.
- [ ] Target maintenance slice: keep `scripts/browser-vm-target-refresh.sh` as the renewable, idempotent target-refresh path before release handoff. It refreshes installed Browser VM helpers and guest initrd/rootfs script artifacts without requiring a full Rust build toolchain, preserves `browser-vm/initrd` and `browser-vm/rootfs.ext4` symlinks, creates timestamped backups, and supports `--verify-only` drift detection. The optional `--guest-control-bridge-bin` path can refresh a prebuilt Linux guest-control bridge binary inside rootfs; broader compiled guest changes still require `scripts/setup-source-home.sh` or a rebuilt/restaged Browser VM rootfs. After any Linux full setup or binary replacement, restart/prove the source-home front door with `scripts/linux-source-home-restart.sh` so the gateway does not remain down after stale-host exit; Linux source-home setup must also install session TURN credentials when the Browser VM uses WebRTC remote display and must preserve an existing remote Browser VM control config on non-KVM gateway hosts. Prove it with the local fixture, `scripts/setup-source-home-browser-config-smoke.sh`, target `--verify-only`, `scripts/linux-source-home-restart-smoke.sh`, and `scripts/jetson-browser-runtime-audit.mjs` with explicit target arguments.
- [ ] Browser runtime proof slice: prove the same WebRTC-only Browser contract through Mac VZ and Jetson crosvm adapters. Source-home generated VM control capacity is still one active page per control service; simultaneous local/remote Browser use must be proved through separate runtime/control-service lanes, not by treating one VM service as multi-page. The older two-open proof was a capacity-rejection/orphan-cleanup gate, not true VM concurrency. Current hardening adds VM control lifecycle status, pending-launch cleanup, warm idle/hibernated VM status, autostart/prewarm proof, and longer control request budgets. Required remaining evidence is deliberate multi-page VM capacity if that becomes a release requirement, long-hold sessions, frame continuity, page/control heartbeat, reconnect behavior, explicit close/orphan cleanup, Home-authorized active Jetson page/crosvm evidence, and operator capacity/status/resource receipts. The 2026-06-25 direct Jetson VM-control proof succeeded for `https://ela.city/` with Runtime-only networking plus `audio=true` and `video=true`, and the strict Jetson runtime audit passed while that page was active; this is a substrate/runtime proof, not a Home-authorized product Browser journey. The non-KVM server is now restored as a gateway/remote-engine consumer through the Mac `browser-vm-remote-vz-launcher`, and `scripts/browser-ela-city-protected-content-open-smoke.sh` passes against `https://elastos.elacitylabs.com/apps/home/` for the known protected `ela.city` route; this proves open/close and advertised audio/video, not decoded-frame continuity or manual audible audio. This server must not grow a non-KVM local-browser fallback that bypasses the contract.
- [ ] Browser remote-engine media slice: remote Browser Engines must resolve media through a trusted Service/Carrier path before returning a WebRTC display session. The Browser UI can choose Browser Engine and Browser Exit services, but it must never expose or require engine-local IP/TURN endpoint reasoning. The 2026-06-26 server-to-Mac proof shows answer signaling works. The server-owned TURN experiment proved only the browser-client side could gather relay candidates; the Mac engine still returned zero engine candidates, so the unused server TURN daemon was stopped and remote-VZ source-home config now refuses to inherit local VM ICE/media env. The live server labels the configured remote-VZ adapter honestly through safe `backing_substrate` metadata and no longer hides it behind a generic automatic engine label, but that is only identity/UX cleanup; it does not create a provider-backed People/Services Browser Engine route or solve first-frame media.
- [ ] Make the Browser remote-Exit stream reuse a Runtime-owned Carrier endpoint, or close any separately owned endpoint explicitly before it is dropped. `open_browser_carrier_stream` still creates a short-lived endpoint, unlike the corrected provider invocation path. Keep this in the Browser transport slice; it does not belong in collaboration closeout.
- [ ] Browser Mac/server simultaneous-use slice: one Browser page on this server through the Mac Browser Engine and one Browser page on Mac must be able to run at the same time. Current intended topology is one active page per runtime control service, not one global Mac Browser limit: this server uses its remote-VZ control service and the normal Mac data dir, while Mac-local Home uses `elastos-mac-test-home` with its own control service. Both Mac data dirs must share the same Mac runtime TURN env instead of racing two TURN daemons on port `3478`; `setup-source-home.sh` now honors `ELASTOS_BROWSER_RUNTIME_TURN_ENV` for that. Remaining proof requires Playwright or an equivalent Mac-local product smoke plus a held server-remote open after the cross-runtime media bridge exists.
- [ ] Browser wallet/product UX slice: prove Browser wallet dapp flows through Runtime-mediated Wallet/Inbox authority, including the known `ela.city` buy-result mismatch, EIP-1193 `eth_sendTransaction` return/receipt shape, account discovery/chain switching, and explicit audio/video/input manual evidence. Run `scripts/auth-wallet-focus-smoke.sh`, Browser wallet smokes, and the manual UX report before any Browser product-readiness claim.
  Current source smokes must be rerun before release proof; manual audio/video/input UX evidence remains open.
- [ ] Installed Home/device proof slice: prove installed `elastos -> Home -> app -> Home` on Mac and Jetson, including live `/apps/home/`, app launch/focus/close, return-home behavior, provider manifest availability, and no source-tree-only assumptions. Keep host adapters behind Runtime contracts instead of branching product behavior per machine.
- [ ] Release package/registry slice: stamp or verify provider component checksums, keep the current `home` publish preflight/dry-run receipt valid, verify installed provider manifests on the target data dirs, decide whether Browser VM helper/rootfs/initrd artifacts remain source-home generated via `scripts/setup-source-home.sh` plus `scripts/browser-vm-target-refresh.sh` or become explicit `components.json` release components, publish the approved version's binary/artifact set so no-override public installed-path smokes use current code, and make changelog/release claims match only the proofs that passed on real target hosts.
- [ ] Final entropy slice: remove only proven-unused reconciliation leftovers, stale display paths, stale work logs, duplicate truth surfaces, generated artifacts, and target backups after they are either archived or intentionally retained. Do not add compatibility shims for removed `runtime_frame`, `diagnostic_frame`, screenshot, image-polling, or host-specific browser paths unless a current shipped caller is proven.

### 1. Blockchain quadrant: identity, wallet, auth, node capsules
- [ ] Enforce the blockchain quadrant contract in code before UI: runtime principal, verified proof bindings, short-lived session grants, scoped capabilities, provider-mediated effects, signed audit, and fail-closed behavior.
- [ ] Keep `scripts/wallet-product-safety-smoke.sh` green before release publish. It is the product-level Wallet safety gate for MetaMask multi-account link/remove, passkey-gated built-in account delete and recovery-key export/import, WalletConnect disabled without pinned operator config, Ledger hidden until implemented, and no hosted Browser UniSat injection path.
- [ ] Make recovery semantics impossible to misunderstand before release publish: System's `Download Recovery Kit` must export one password-protectable full bundle containing the principal-owned Home/user data root plus every recoverable built-in Wallet key for that principal. Individual `elastos.wallet.recovery-key/v1` export/import remains an advanced per-account escape hatch. External wallets such as MetaMask, WalletConnect, Ledger, Essentials, and UniSat can only restore links/metadata because their private keys live outside ElastOS. Deleting a built-in wallet must warn when no full bundle or individual Wallet key has been saved, and the main Wallet view must offer both `Create account` and `Import Wallet key` without sending users to Settings.
- [ ] Keep first-run recovery honest across both states: a fresh Recovery Kit can unlock a surviving root immediately, but empty-machine recovery for a kit created before the later random Profile key exists still needs a separate source repair.
- [ ] Keep the capsule boundary canonical: capsules invoke typed ElastOS Bus
  resources for Wallet, DID, Chain, and other effects. Carrier is an optional
  authenticated transport adapter behind those resources, not the capsule API.
- [ ] Keep app/viewer/content capsules away from wallet RPC, node RPC, raw HTTP ports, chain SDKs, and private-key material; only wallet/node provider capsules may hold those authorities.
- [ ] Keep principals, proof bindings, and sessions separate. Principals are people, agents, devices, capsules, and providers. Wallet addresses, BTC addresses, `did:key`, and `did:elastos` are proof bindings. Sessions are ephemeral grant contexts, not identities.
- [ ] Separate signing roles explicitly: device DID, human/persona DID, agent DID, capsule/provider DID, publisher DID, optional object/head identity, and session grant. Define which identity signs launch grants, Carrier envelopes, package manifests, published objects, credentials, global name claims, and access rights.
- [ ] Build authentication as proof-bound runtime sessions: Home, browser pairing, and app launch grants must be non-delegatable capabilities bound to principal + proof binding + device/browser + capsule + expiry, not route shape or iframe placement. Passkey is the required default human proof; wallet, BTC, ELA, EID, and UniversalX are adapters linked after a Runtime principal exists.
- [ ] Complete self-sovereign guest data after the guest self-registration slice: guests create their own passkey, principal, and downloadable Recovery Kit through Home/System authority; keep proving admins never receive guest authenticator, recovery phrase, or principal data-key material.
- [ ] Eliminate remaining shared `localhost://Users/self` assumptions in favor of session-principal roots. Any Home-backed launch, shell/supervisor launch, WASM bridge, attached/native CLI path, or provider bridge that touches user-root state must receive verified principal authority through a signed non-delegatable grant and must fail closed for raw `principal_id`, raw `home_token`, explicit foreign roots, or provider-role user scope.
- [ ] Complete explicit passkey recovery/reassignment UX for `localhost://Users/<principal-root>` roots. A verified Recovery Kit is emergency root authority: it can recover an account under a new passkey, revoke/replace old passkey-root bindings, reissue the Home/System session, restore included built-in Wallet keys, and record signed audit. Keep `ELASTOS_HOME_TOKEN=<signed-token> scripts/recovery-kit-live-smoke.sh` as the live proof hook, and add the pre-login `Recover existing account` path so users do not need to understand temporary guest accounts.
- [ ] Extend principal-root encryption coverage behind `elastos.principal.root-protection/v1` before claiming all user data is safe at rest. Every new `localhost://Users/<principal-root>` writer must use the runtime/provider protected storage helper or fail closed, including attached/remote bridges and future Browser profile state.
- [ ] Make recovery and migration user-friendly and quantum-conscious behind `elastos.recovery-kit/v1`: add client-side WebAuthn PRF wrapping without sending raw PRF output to the runtime, add DID-envelope unwrap or rewrap before claiming DID-only recovery, add `did:elastos`/EID resolver-backed proof verification, and implement future ML-KEM/ML-DSA/SLH-DSA/HQC envelopes.
- [ ] Keep WebAuthn RP policy operationally explicit: production Home needs a stable HTTPS RP domain, local development uses `localhost` as a separate passkey world, and native/mobile hosts need an explicit host-auth adapter rather than a header-based bypass.
- [ ] Add WalletConnect as a dedicated connector capsule, not as authority inside ordinary apps and not as raw SDK state inside app capsules. `wallet-provider` owns proof bindings, approvals, receipts, and audit; `wallet-walletconnect` owns only Reown/AppKit browser UX plus an operator-pinned local adapter. Do not commit a bundled default Reown Project ID; official deployments and independent operators must pin their own runtime config and local SDK asset before the visible connector path is enabled.
- [ ] Move the inherited hardcoded `wallet_connector_evm_chains` metadata out of Runtime gateway code and behind the provider-owned chain metadata boundary. Until then, treat the ESC/Base names, native currency fields, and RPC URLs returned by that helper as explicit provider-ownership debt, not Home, shell, or connector authority.
- [ ] Add Essentials/ELA only after the pinned WalletConnect connector contract exists: use Essentials or Elastos Wallet JS SDK for ELA mainchain signing, with EID treated as an optional identity/proof adapter for credentials, recovery, publisher identity, verified service endpoints, and DAO operations, not as a default chain network.
- [ ] Complete real-wallet evidence and proof-strength policy for external BTC verification. Managed Bitcoin remains native P2WPKH. Source tests currently cover external BIP-322 simple P2WPKH/P2TR verification and Bitcoin signed-message P2PKH/P2SH-P2WPKH verification, but they are not real UniSat compatibility evidence. Pin real UniSat evidence for every claimed path and define the weaker capability policy for legacy signed-message proofs before making product or privileged-capability claims; keep all Bitcoin node credentials/ports inside `chain-provider`.
- [ ] Treat UniversalX/Universal Accounts as optional onboarding and transaction UX adapters. They must never mint runtime principals, runtime sessions, or privileged capabilities directly.
- [ ] Continue converging Wallet UX around one user mental model: Wallet -> Accounts -> balances/assets/activity -> approval methods. Add token/NFT asset reads, richer activity history, oracle/provider-backed price feeds, and fully wired send signers without exposing raw wallet RPC, chain RPC, node ports, HTTP/Web APIs, connector SDK authority, or private-key material to ordinary capsules. External HTTP pricing must stay disabled until it appears as an Inbox request and an admin explicitly approves the local price-source policy; actual HTTP price fetches must remain audited; the durable target is a typed oracle/price provider with signed receipts.
- [ ] Keep DID/name/CID semantics explicit before coding beyond the first auth slice: `did:key` is the local device/node DID, passkey principals are local account roots, `did:elastos`/EID is the future global account/credential/namespace path, and CIDs identify immutable content graphs. Do not add `did:localhost` or treat local handles such as `alice` as global identity; globally scarce names need a chain/registry claim path that prevents double-claiming.
- [ ] Keep blockchain UI limited to passkey login, Wallet-owned accounts/approval methods, dedicated connector capsules, and System diagnostics backed by typed provider operations. New node write/broadcast/lifecycle controls must not appear until provider manifests, capability schema, approval/audit policy, and verification commands cover them. System owns account policy and diagnostics; Wallet/Inbox own wallet accounts and approval review; connector capsules are explicit wallet-adapter surfaces; ordinary app/viewer/content capsules must not reference raw wallet, chain, node, RPC, WalletConnect, MetaMask, or blockchain-provider authority directly.

### 2. Home environment
- [ ] Keep `home` as the host/front-door bridge ID, with selectable shell
      identities limited to `home-gui` and `home-cli`; visible product language
      is `Home`; legacy `home` active-shell input must resolve to `home-gui`
      and never persist as a shell value.
- [ ] Extend the runtime-owned Home contract beyond identity + app catalog: Library browsing, runtime health, capability prompts, and attach/focus semantics.
- [ ] Expand `System` beyond identity + app inventory into a real system surface.
- [ ] Prove one truthful `Home -> System -> app -> focus/close -> Home` manual loop, then decide the first non-browser attachment contract.
- [ ] Keep the default Home path compatible with macOS and Windows by avoiding KVM-only assumptions.
- [ ] Remove remaining donor/KVM-only assumptions from scripts and runtime special cases.
- [ ] Complete the launch ingress parity review. Runtime already issues signed,
  non-delegatable launch grants; route and attachment fields describe the
  projection. Verify that each ingress derives authority from the grant.
- [ ] Add an explicit runtime/manifest exposure contract for Home, gateway, and shared surfaces so internal-only and external-only objects do not depend on name-based filtering.

### 3. Home front-door boringness
- [ ] Prove one boring installed `elastos -> Home -> app -> Home` path on Jetson and WSL.
- [ ] Keep tightening dashboard navigation, return-home behavior, and single-owner TTY/session rules until target-machine proof is boring.
- [ ] Keep unfinished surfaces out of the main live path unless they launch from Home and return cleanly.
- [ ] Rehearse and simplify the Home/People/Spaces/System story so the front door feels useful without internal-runtime narration.
- [ ] Extend `elastos.runtime.services/v1` beyond local configured-provider cards and conversation offers: remote Exit, storage, relay, model, and hosting offers must arrive as provider-backed `elastos.service.offer/v1` records through People/Carrier, and enabling one must create/select a principal-scoped provider grant instead of giving capsules direct People-state authority.
  - [ ] Model Provider subtask: keep one typed `model-provider` contract and
    complete it in this order:

    1. evaluate Qwen3.5-9B Q4_K_M as the Mac baseline and PrismML Bonsai 8B Q1
       as the low-memory comparison; keep Qwen3.8-27B and Bonsai 27B as later
       benchmark candidates
    2. install and prove the `model-provider` llama.cpp engine lifecycle on
       macOS Metal, including verified artifacts, health, limits, stream,
       cancel, restart, shutdown, and orphan cleanup
    3. prove hosted adapters locally through the current Chat Completions seam,
       then add the provider-internal OpenAI Responses API adapter and prove
       explicit provider, cost, privacy, limits, requested selector, resolved
       model, and fallback facts
    4. add optional `elastos.service.offer/v1` publication with an
       operator-selected offer, Runtime policy, and a principal-scoped grant
    5. run a full Runtime on Jetson and prove the signed model service over
       Carrier before considering a smaller provider host

    Runtime keeps publication, grants, quotas, selection, routing, and audit.
    The provider keeps backend URLs, credentials, process details, and topology
    private. Model artifacts remain immutable content installed through the
    content-provider path.
- [ ] Promote principal-owned Appearance state into a DID-anchored profile/settings object that syncs through Carrier/provider policy and projects back into `localhost://Users/<principal-root>/.AppData/ElastOS/Home/Appearance/...` per trusted device.
- [ ] Keep `Apps` as the public catalog term and `capsules` as the internal/runtime term; do not expose both as competing public nouns.
- [ ] Keep settings in `System`; keep files, documents, and provider-backed storage in their owning apps instead of recreating a generic System Storage section.
- [ ] Decide the explicit home-return contract for native and non-native chat surfaces.
- [ ] Split Home surfaces cleanly into launchable apps, site/share actions, and support assets instead of mixing them in one Apps list.
- [ ] Keep only shipped, installable, launchable, and useful items in `Apps`; demote or hide unfinished catalog-only entries until they earn real Home actions.
- [ ] Make `MyWebSite` useful from Home with a real local preview path plus a first-class `Go public` action, not just long notices.
- [ ] Make `setup --profile demo` install the app capsules Home honestly advertises, or stop advertising them there.
- [ ] Decide whether blocked apps should be hidden entirely from the main Apps surface or moved into an explicit install/setup section.

### 4. Release / install / update coherence
- [ ] Lock interactive-launch, stale-runtime, and stale-support-asset regressions with explicit coverage.
- [ ] Extend outsider proof beyond local x86_64 until Jetson/WSL evidence is equally solid.
- [ ] Keep `scripts/public-install-identity-smoke.sh` in scope as the DID-backed People/profile contract for public install proof.
- [ ] Keep `scripts/public-install-operator-smoke.sh` and `scripts/public-install-home-frontdoor-smoke.sh` in scope as installed public front-door/operator proof.
- [ ] Keep `scripts/audit-linux-runtime-portability.sh` in scope as the public Linux runtime portability proof.

### 5. Truth surfaces and anti-drift
- [ ] Remove duplicated volatile facts such as scattered versions, metrics, and proof transcripts from durable docs.
- [ ] Simplify `components.json` so installable first-party components do not live in two competing top-level registries (`capsules` and `external`) with duplicate names. Keep one canonical component record and derive release/setup views from it.
- [ ] Replace the `/tmp/elastos/storage` serve default in `elastos/crates/elastos-server/src/run_cmd.rs` with a stable data-root path. Prove an ordinary restart keeps the same owned state before changing installed Homes. This is a later code slice.
- [ ] Collapse or clearly document the two capsule source roots: root `capsules/` holds most first-party capsules, while `elastos/capsules/` still holds `shell` and `localhost-provider`. The repo should expose one obvious source layout for developers before the next release line.
- [ ] Keep `PRINCIPLES.md`, docs, and command surfaces aligned through fail-closed checks instead of periodic prose cleanup.
- [ ] Encode the proof-first and command-surface guardrails in durable repo docs so agents do not keep reinventing launch models or overstating proof.
- [ ] Reject plans that add public UI, protocol bridges, provider behavior, or blockchain hooks before the underlying principal, capability, package, or space contract is explicit and testable.

### 6. Site / publication surface
- [ ] Keep `MyWebSite`, publication, channels, activation, and rollback on one coherent local-first path.
- [ ] Evolve site/publication state toward cleaner resolver-owned system-service objects.
- [ ] Make the combined publish + host refresh + live deployment ceremony deterministic and easy to verify.

## Next

### Capsule ABI stabilization
- [ ] Use the capsule contract in
  [docs/CAPSULE_MODEL.md](docs/CAPSULE_MODEL.md) as the shared
  acceptance contract across branches. Keep ownership narrow: ESP owns Bus v1
  and shell projections; component-runtime hardening owns admission and resource
  enforcement; capsule-package trust owns bundle identity and interface
  compatibility; runtime lifecycle owns resident execution, cancellation, and
  streams; content availability and WebSpace work own portable state; Carrier
  owns authenticated transport; Mandate owns delegated authority. Branch plans
  should link to this contract and record only their delta instead of copying it.
- [ ] Make Component admission enforce each verified manifest's declared memory,
  compute/fuel, activation-time, and instance bounds within Runtime policy
  ceilings. Add exact-limit and over-limit tests, and do not report the current
  fixed 128 MiB/fuel settings as per-capsule resource enforcement.
- [ ] Bind Component Bus identity to distinct Runtime-verified principal,
  capsule, session, device/proof, and launch-grant records. Remove the current
  capsule-id-as-session placeholder and add cross-principal, stale-session, and
  provider-role negative tests before the identity context is treated as proof.
- [ ] Define the durable re-instantiation compatibility contract: full signed
  bundle root, publisher and revocation state, interface versions, immutable
  dependency closure, compatible Runtime range, state schema and migrations,
  availability, and install/update/migration receipts. Prove the same artifact
  can be admitted on a second compatible node without its original app store or
  source checkout before claiming indefinite portability.
- [ ] Design `elastos:bus@v2` only when a concrete product Component requires
  resident lifecycle or streams. Keep `elastos:bus@v1` bounded and immutable;
  do not add stream, lifecycle, cancellation, capacity, or object-handle
  semantics until they share one Runtime authorization, provider, audit, and
  cleanup path.
- [ ] Port one small first-party product App to `elastos.component/v1` before
  claiming Component/Bus product adoption. Keep the conformance fixture and
  authoring template described as contract proof until that migration passes
  installed lifecycle, authority, state, and UX evidence.
- [ ] Implement the Browser Capsule architecture in [docs/BROWSER_CAPSULE.md](docs/BROWSER_CAPSULE.md): one Browser/Net/Exit ABI above platform-specific engine adapters, with no ambient host internet, no raw sockets, no raw DNS, no direct Runtime API exposure to web pages, no raw wallet/chain/storage authority, and profile/bookmark/download state rooted under the active principal.
- [ ] Move Browser profile persistence into principal-owned `localhost://` state: cookies, localStorage, IndexedDB, service workers, permissions, bookmarks, history, and downloads must live under `localhost://Users/<principal>/BrowserProfiles/<profile>/...` or an equivalent provider-owned encrypted root, never a shared hosted-Chromium/container profile. This must preserve dapp sessions across refresh/restart, prevent admin/guest leakage, support Recovery Kit/migration, and be covered by tests proving two principals cannot read or mutate each other's browser profile state.
- [ ] Define the Net/Exit provider contract separately from browser UI before improving the visible Browser surface. Runtime must validate Browser stream requests through Net, hand them to Exit only through explicit capability policy, keep HTTP-fetch proxying as a constrained compatibility/diagnostic capability, block LAN/private IP access by default, and hide private adapter/relay IPC descriptors from Browser UI responses.
- [ ] Treat the current `browser` capsule as a Runtime Browser proof, not a final general-purpose browser. It may render public HTTPS pages through an operator-configured Exit policy, but it must not claim final native/microVM isolation, raw wallet compatibility, general off-box Browser support, or product-quality media until those proofs land.
- [ ] Keep the visible Browser on the WebRTC engine path. Address requests call
  `/api/apps/browser/open`; Runtime reserves streams through Net/Exit and gives
  the Browser projection page-scoped WebRTC session and control handles.
  `elastos://net/http` remains diagnostic-only.
- [ ] Finish the Browser Engine Adapter behind the internal `elastos://browser-engine/*` contract. Engine adapters must use operator-approved supervisor commands, Runtime-mediated Exit streams, no direct host TCP/DNS/HTTP, no wallet injection, no chain RPC, no raw host-network authority, and fail-closed display/input proofs.
- [ ] Keep `browser-playwright-engine` diagnostic-only. It may exercise Runtime Exit, display-session, input, and wallet-bridge contracts, but it must never be treated as product Browser runtime or allowed to claim product audio/video acceptance.
- [ ] Keep product Browser providers behind one `elastos.browser.display-session/v1` product-compositor contract. A candidate must prove audio, video, display coordinate size for datachannel input, navigation, wallet mediation, `direct_network=false`, cleanup, media stress, and manual UX through [docs/BROWSER_PROVIDER_BAKEOFF.md](docs/BROWSER_PROVIDER_BAKEOFF.md). Browser wallet chain selection must come from Runtime Wallet defaults, never website hostnames or dapp-specific rules.
- [ ] Keep Browser dapp wallet compatibility split by authority class: account discovery and chain switching stay inside the constrained Runtime-mediated EIP-1193 bridge; read-only chain calls route through typed `chain-provider` reads with audit; signing, typed-data signing, and transaction effects create Wallet/Inbox approvals before any result reaches the page. Track the remaining `ela.city` buy-result mismatch as an open Browser wallet-compatibility blocker: the buy approval can execute on-chain and unlock content, but the dApp still surfaces failure for another return-path reason. Before release closeout, capture the exact dApp-visible error, add a focused regression around the EIP-1193 `eth_sendTransaction` resolution/receipt shape, and ensure the Browser bridge returns the same transaction hash/status semantics a normal injected wallet would return after Wallet/Inbox approval.
- [ ] Generalize Browser/capsule effect governance after the current Browser open/read audit slice: every external effect must emit request/completion audit records, and any standing, time-bound, or permanent approval must be represented as a scoped Runtime capability/grant instead of an app-owned bypass. Browser opens/read-only chain calls, Wallet external HTTP price fetches, and System-triggered chain node lifecycle control now have audit coverage; continue the same pattern for provider installs and future approval grants.
- [ ] Add Browser viewport-resize acceptance proof before calling the Browser normal-browser-equivalent. The proof must fail closed unless the remote display adopts the requested compositor size, preserves page ratio, keeps input coordinates aligned, and avoids stretch/letterbox artifacts at common Home window sizes.
- [ ] Add a Runtime-owned Browser tab/session model before exposing real multi-tab UX. Until then, popup and `_blank` navigation must stay in the current Browser page so the user is never moved into a hidden hosted-engine target. The eventual tab strip must switch explicit Runtime page ids, keep each page's wallet/profile/session state principal-scoped, and preserve Browser/Carrier/provider mediation for every tab.
- [ ] Replace remaining fixed-interval app polling with one Runtime/provider subscription or stream per signed Home session where the app needs realtime behavior. Home prefers one Runtime-owned SSE stream at `/api/apps/home/events/stream`, with `/api/apps/home/events` kept as a compatibility long-poll fallback, and forwards scoped events into child app frames. Wallet and Inbox consume that path. Chat Room does not yet consume Home events and still polls every second in shell mode; do not claim the planned 30-second safety poll until the event-driven refresh path is implemented and tested. Scoped provider events must not force full Home summary refreshes unless they change shell-visible state: Wallet request/approval events refresh Home summary for the top-nav attention badge, while ordinary Wallet balance/activity updates, Browser state, and Chat changes go to their frames. Home summary refresh is reserved for boot, shell-relevant Home/Inbox/Wallet approval events, foreground visibility, and session refresh only. Home must not run a periodic `/summary` poll. Inbox's safety poll is 5 minutes, not a realtime mechanism. Event cursors must remain scoped and non-volatile: heartbeat fields such as Chat `last_seen_at` must not emit app-change events or cause refresh feedback loops. The durable contract should evolve this into provider-backed scoped events such as wallet request created/completed, inbox changed, Browser page state changed/lost, balance changed, system policy changed, and chat room changed. Browser heartbeat may remain as low-rate liveness only; it must not become UI-state transport. Any remaining generated app loops still need the same subscription/stream pass so desktop dragging and multi-window use are not penalized by background polling.
- [ ] Keep Selkies bounded as the current hosted proof path. The live host now uses per-launch Selkies targets instead of the old singleton service, but product completion still requires `scripts/browser-objective-audit.mjs`, provider bake-off evidence, media evidence, and manual UX evidence before Browser/audio can be called complete.
- [ ] Keep `scripts/browser-objective-audit.mjs` as the Browser completion gate. It must pass architecture checks, no-fake-media checks, accepted hosted or native product-provider evidence, and hash-bound manual UX evidence before Browser/audio can be called complete.
- [ ] Keep YouTube/audio readiness as a product stress gate, not a fixture-only proof. Product acceptance requires audible playback, address-bar stability, typing, scrolling/click fidelity, wallet connect, no raw authority, cleanup, and arbitrary-site behavior through the selected provider.
- [ ] Keep hosted operator scripts bounded and explicit. Selkies-specific scripts are proof/operator tools, not general Browser architecture, and public `gst-py-example` must not be wired directly into ElastOS Browser.
- [ ] Build the first native Linux/Jetson browser proof after the server/headless proof: CEF/Chromium or Chromium-in-microVM with a real compositor/audio/video surface, native supervisor launch, loopback proxy to Runtime Exit relay IPC, direct TCP/DNS/HTTP denial, DNS leak test, LAN/private IP block test, and manual public-web/Glide dapp proof through Runtime-mediated wallet requests. Playwright remains diagnostic/test infrastructure only, not product browser runtime.
- [ ] Add Windows, macOS, and Android browser engine adapters only behind the same Browser/Net/Exit ABI. Use WebView2/CEF on Windows, CEF or constrained WKWebView work on macOS, and Android WebView/GeckoView on Android only after host-auth, passkey origin, and app network policy are explicit. Treat Servo, WPE WebKit, and full WASM/WASI browser engines as R&D options behind the same ABI, not as the first product target.

### Four-quadrant runtime balance
- [ ] Balance the next phase across the four ElastOS quadrants instead of over-investing in one layer:
  1. **PC2/Home**: user front door, object browser, install/launch UX, spaces and people views
  2. **Runtime**: trusted core, principals, sessions, package verification, interface contracts, capability routing
  3. **Carrier**: authenticated object/message/stream transport, discovery, sync, replication, content delivery
  4. **Blockchain**: DID/EID, wallet signing, provenance anchors, publisher identity, optional receipts/licensing
- [ ] Use this order for future plan reviews: first prove the runtime contract, then expose the PC2/Home UX, then route through Carrier/provider transport, then add blockchain anchoring only where identity/provenance/approval needs it.
- [ ] Finish passkey-first authority as the first balancing move:
  1. PC2/Home: refresh-safe session hardening, recovery, and approval UX
  2. Runtime: principal-root storage adoption, revocation, audit, and agent delegation
  3. Carrier: session attach and delegated capability envelopes without leaking host/browser identity
  4. Blockchain: wallet/DID proofs as adapters linked after the Runtime principal exists, never as Home login roots
- [ ] Build Spaces/network drives after the auth slice:
  1. PC2: `People`, `Spaces`, and shared-drive browsing without exposing transport names as product truth
  2. Runtime: mount records, object heads, ACLs, watch/sync APIs, and resolver-owned WebSpace traversal
  3. Carrier: discovery, sync, replication, shared-state updates, and content delivery
  4. Blockchain: optional ownership/provenance anchors for space heads and published shares
- [ ] Build SmartWeb content availability as the default publication behavior:
  1. PC2: publish/open/status UX says whether an object is local-only, syncing, network-available, or repair-needed
  2. Runtime: content capability schema, availability receipt verification, audit, and provider routing
  3. Carrier: peer discovery, replication coordination, signed object exchange, and repair signaling
  4. Blockchain: optional provenance, dDRM rights, and later storage incentive settlement from signed receipts
- [ ] Evaluate PC2 Kubo/IPFS Cluster/supernode replication as the first real `availability-provider` backend after the provider contract is stable; keep `elastos://content/*` as the capsule-facing contract.
- [ ] Build capsule publish/install registry after Spaces/network drives:
  1. PC2: install/pin/unpin UX, trusted/untrusted publisher state, and app catalog actions that all work
  2. Runtime: signed bundle identity, whole-package verification, interface/version contracts, install receipts, update policy
  3. Carrier: package/update distribution and peer discovery for trusted sources
  4. Blockchain: publisher identity, provenance receipts, and optional license/payment hooks without making token mechanics the core model
- [ ] Keep Marketplace remote install/update/uninstall gated until the signed install contract exists. Marketplace may browse installed and verified Runtime apps, show details, and open installed Home launch targets, but remote actions need signed app manifests, publisher identity, install/update/removal receipts, provider policy, payment receipts where required, protected-content rights/custody/decrypt policy, and Home/capsule change events before one-click install is exposed. Loose repo/dev folders can remain Home/dev targets, but they must not be presented as remotely installable Marketplace apps.
- [ ] Do not prioritize rich DRM economics, DeFi/BtcFi, Android box specifics, or literal Capsule-NFT mechanics before the package identity, principal, space, and provider contracts are real.

### Runtime primitives missing for the PC2 world-computer model
- [ ] Replace hardcoded `Users/self` assumptions with first-class principals: passkey-owned user roots, user DID, device DID, personas, agents, active session, and capability tokens bound to principal + capsule + session.
- [ ] Add authenticated Carrier envelopes as an optional transport adapter
  behind typed ElastOS Bus resources: sender DID, object identity, signature,
  capability context, replay protection, and verified delivery status. Keep raw
  gossip/transport as an explicit unsafe provider-level lane.
- [ ] Keep the `object-provider` / `content-provider` ontology stable while completing the remaining World Computer object/content work: `object-provider` owns mutable principal-root objects, and `content-provider` owns published content identity, availability, and Carrier-backed delivery authority.
- [ ] Extract pure object-provider core out of `elastos-server::library` into a smaller provider-core crate when modularity becomes the release bar: preserve the existing `object-provider` capsule/API boundary, move principal-root object request handling, path rules, archive/event helpers, and tests without changing Library behavior, and keep publish/share/availability authority separated through `content-provider` and Runtime coordination.
- [ ] Keep Public placement and Published content separate in every Library/Home/Spaces surface: `Public` is a user-facing placement/projection under the active principal root, while `published_cid`/`elastos://<cid>` is the only public content-link truth. Do not add hidden auto-publish side effects for rename/move/copy/upload into Public; if auto-publish is desired later, make it an explicit user policy prompt backed by content-provider receipts.
- [ ] Add signed package identity for every installable capsule: manifest hash, full bundle hash/Merkle root, publisher DID, signature chain, interface descriptors, and install/update receipts.
- [ ] Add an interface registry primitive: signed interface descriptors, semantic versions, required/provided capability schema, compatibility resolution, and fail-closed launch when required interfaces are missing.
- [ ] Complete wallet/EID/chain providers behind the runtime boundary. The runtime should expose capability-gated signing, approval, credential, node-read, proof, broadcast, and provenance operations; it should not embed chain business logic.
- [ ] Keep network-drive/provider operating systems outside the trusted core. The runtime owns verification, capability routing, and audit; provider capsules/services own Telegram/Nostr/Matrix/Facebook/IPFS/Carrier-specific behavior.

### WebSpace / World Computer contract
- [ ] Clarify the relationship between rooted localhost paths, `elastos://...`, and mounted WebSpace views without freezing syntax too early.
- [ ] Make the Spaces UX model explicit before expanding Library roots: `Home` is the friendly alias for the active principal's local `localhost://Users/<principal>` space; a future `Localhost`/`This Device` Space may expose the same authorized principal tree and selected system roots, but never raw all-host data or other principals. `elastos://` should remain the global content/capability namespace, not a writable file path. A future `elastos://vault` (name TBD) should be an encrypted, DID-anchored, provider-backed replicated object space that can fork/sync selected local objects; quota/accounting applies there and to published/federated storage, not to ordinary local-only `localhost://` bytes.
- [ ] Define the CAS object model so paths stay the comfort layer rather than the real identity model.
- [ ] Keep capsule execution substrate (`type`), product role (`shell`/`app`/`viewer`/`provider`/`content`), and launch exposure as separate runtime concepts instead of letting one field imply the others.
- [ ] Document and enforce the object/capsule/space split consistently across UI copy, manifests, runtime docs, and shell/catalog surfaces.
- [ ] BLOCKER - production multi-peer availability/storage markets require real external infrastructure before this can close: production independent provider-network quota-ledger federation beyond the configured bounded endpoint quorum, production network-wide abuse throttles/banlists/abuse ledgers beyond the configured bounded abuse-control endpoint quorum, production federated operator fleet dashboards/UI/peer-health subscriptions beyond the current provider-local dashboard plus configured alert-exchange endpoint, production cross-runtime peer reputation trust policy, third-party attestations, revocation, and fleet-wide reputation exchange beyond the configured Carrier peer-attestation endpoint quorum, production storage-market offer/pricing/SLA execution beyond the configured storage-market endpoint-quorum admission gate, repair-fleet worker attestation/SLA/settlement beyond configured dispatch quorum, and live settlement/escrow execution.

### Collaboration and messaging
- [ ] Earn IRC only as an explicit packaged path with honest runtime prerequisites and proof.
- [ ] Keep the old Services remote-Exit social/contact path isolated as a separate legacy migration; it must never feed People identity or contact authority.
- [ ] Split People/Contacts from Services offers in the later read-model slice; `HomePeopleSummary` still carries Services offer fields today, but the Profile-backed People path should keep them empty rather than treating them as contact truth.
- [ ] Complete the canonical collaboration blockers and installed journey in
  `Now` before adding another messaging surface. Do not reopen a parallel
  Profile, discovery, delivery, or acceptance checklist here.

### Documents and Library
- [ ] Add import/fork flows for immutable `elastos://<cid>` document revisions through the same provider contract.
- [ ] Future generic archive dependency approval: only after a format-specific review passes, enable an extra non-tar/non-zip family through the existing provider-owned archive list/preview/selective-extract/WebSpace policy contract. Current branch support for ZIP/tar/tar.gz/tgz browsing, preview, selected import/extract, WebSpace archive policy, and Archive UX is complete; unsupported generic families remain policy-gated by design.
- [ ] Unify the markdown packaging model so local documents, viewer/editor content, and `elastos share` do not keep using three different markdown stories.
- [ ] Decide the first collaborative document core intentionally; prefer a Rust/WASM CRDT evaluation (`Yrs` first, `Automerge` second) over ad hoc editor glue or a direct port of external JS products.
- [ ] Keep keystroke-level local editing local-first and low-latency; Carrier should carry remote sync/share/collaboration updates, not gate every same-runtime write.
- [ ] Keep the remaining implementation order explicit:
  1. add import/fork/open flows for `elastos://<cid>` revisions through the same provider contract
  2. unify local documents, viewer/editor content, and `elastos share` under one markdown packaging story
  3. add collaboration, comments, and presence on top of the provider/session contract instead of baking sync assumptions into the editor UI

### Inbox
- [ ] Add Inbox coverage for every remaining first-party approval/action flow that can be initiated by a human or an agent. Chat room pairing, wallet approval review, and generic Runtime capability requests are covered; keep extending the same pattern instead of creating app-specific approval surfaces.

### Human/agent parity and design system
- [ ] Extend signed Home/System browser smokes using Chrome/CDP WebAuthn virtual authenticators without creating a login bypass, reusing human session cookies, automating a real personal passkey, or running against remote Home unless explicitly requested with `HOME_VIRTUAL_AUTH_ALLOW_REMOTE=1`. Decide whether Camofox should gain equivalent CDP virtual-authenticator control or Playwright/CDP should remain the signed-session proof runner, then add Browser full-render signed journeys after the Browser provider acceptance gate is unblocked.
- [ ] Add first-class production agent principals behind the same Runtime authority model: a human/admin passkey creates or revokes `agent:*` / `did:key` agent principals, agents sign Runtime challenges with their own keys, Runtime issues short-lived scoped sessions, and high-risk scopes such as wallet signing, Recovery Kit export, account deletion, provider install, node lifecycle, and privileged System changes require explicit human approval through the owning surface: Wallet/Inbox for wallet authority, System/Inbox for runtime policy.
- [ ] Extract the shared capsule token block into a versioned support asset once capsule packaging can import shared CSS without coupling style to runtime authority.

### Trusted content and access rights

Current protected-content source status lives in [state.md](state.md).
The contract is in [Protected content](docs/PROTECTED_CONTENT.md).
The J5 and C6 rows in the [current execution queue](#now) own the acceptance
sequence, with the detailed criteria in sections 8 and 9 of the approved plan.
External cryptographic review remains open before public dKMS or production
confidentiality claims.

PR #15 is source evidence, not a merge target. The retained extraction ledger
covers threshold crypto, recipient-sealed contributions, CEK commitment,
node-local custody, typed player and Creator UX, grant authority, failure cases,
and applicable CI lessons. Current video uses `elacity-player`; document and
3D viewers remain later typed-viewer work.

### Operator and audit hardening
- [ ] Review the legacy-auth migration from PR15 (`0b43da8c`) as a separate
  compatibility decision. It preserves identity records but clears unchained
  audit history and starts a new chain. The current signed-checkpoint policy
  in `docs/AUTH_AUDIT_CHAIN.md` remains canonical. Any replacement must bind
  an explicit operator decision, verified input, retained historical evidence
  and crash-safe recovery before it can change an existing data root.
- [ ] Keep the existing SHA-256 audit chain canonical unless an explicit
  versioned migration is approved. BLAKE3 may remain a content/cache/transport
  choice, but an audit migration must add an algorithm id, canonical encoding,
  golden vectors, a signed transition from the retained SHA-256 head, and
  restart/tamper/truncation tests. Remove branch-plan claims that BLAKE3 audit is
  already implemented.
- [ ] Keep `verify`, `command-smoke`, `installed-command-audit`, and related gates honest and fail-closed.
- [ ] Continue the systematic crate audit through the remaining runtime crates.
- [ ] Review remaining security advisories after the coordinated Iroh 1.0.2
  upgrade, including the status of any retained Hickory exceptions. Keep the
  `bincode 1.3.x` migration as a versioned serialization change with compatibility
  tests. Review Sash's macOS VZ / `elastos-crosvm` Darwin substrate work against
  the current host adapter before any further platform integration.
- [ ] Bound the I/O bridge while reading capsule request frames, before its
  existing parse-size check. Cover oversized and incomplete input and bounded
  cancellation; keep this distinct from the Carrier request framing task.

### Dead code cleanup
- [ ] Re-audit `provider/registry.rs` from current source, not from the stale dead-code list that existed before the 2026-03-31 cleanup. Only remove API surface that is now proven unused on the installed path.
- [ ] Continue the crate-by-crate orphaned-code audit with the same fail-closed rule: delete only after proving the installed path does not use it.

## Long term

- [ ] Evaluate WebAuthn PRF and passkey-derived wallet keys only after passkeys are stable as runtime-session and wallet-provider approval gates.
- [ ] Define the browser host-adapter model without faking Linux parity, using the Browser/Net/Exit ABI above so server, desktop, mobile, and kiosk hosts expose the same capability contract.
- [ ] Define the later Codex SDK agent-execution adapter and its operator
  packaging. Keep it behind typed agent operations and explicit filesystem,
  network, tool, and approval grants. Keep Codex out of model offers.
- [ ] Evaluate public dKMS only after permissioned dKMS has release receipts, share rotation, monitoring, node admission policy, staking/slashing assumptions, and external crypto review.
- [ ] Consider renaming `elastos-server` crate to `elastos-cli`. It is the CLI binary + all commands, not just a server. The current name misleads new developers about what the crate does.
