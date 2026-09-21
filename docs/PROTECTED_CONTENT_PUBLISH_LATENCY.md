# Protected-content publish latency — investigation and fix plan

Investigation date: 2026-09-18. Status: principal latency and replication failure
causes identified; implementation and installed acceptance remain pending.
This report replaces the earlier cold-Carrier-dial diagnosis.

A second measured run on 2026-09-21 reproduces every root cause below, closes the
open question about the DHT topic join, and adds a managed-Kubo idle amplifier.
Read it with this report: [Second measured run](#second-measured-run--2026-09-21-with-stage-instrumentation-in-place).

## Outcome

The measured mint waits for **remote Kubo pins**, then repeats work because the
custody host's availability provider rejects the Runtime request envelope.
For the metadata directory, the fallback import also changes the object manifest,
so it cannot reproduce the requested CID. Sequential candidate processing makes
these costs additive on the Creator request.

The previously unexplained 70 seconds belongs to a **third candidate**, present
in the full gateway log. There is no measured 70-second post-loop delay.
Warming Carrier connections alone will not fix this incident.

Evidence comes from the gateway log, all three custody container logs for the
same interval, persisted content receipts and repair tasks, a read of the exact
metadata manifest through custody C's Kubo API, and the current source. The
investigation made no source changes, restarted no services, changed no peering,
and submitted no mint or chain transaction. Only this hand-off is edited.

## Evidence and scope

The recorded interval is 09:29:43–09:34:02 UTC on 2026-09-18, on macOS with three
local custody containers and the Creator configured for Base mainnet. All times
below are UTC. Container names are `custody-host-custody-{a,b,c}-1`.

The gateway log inspected was `.vscode/logs/gateway.log`; its SHA-256 at inspection
was `82d5112cd73c979c7d3b86021c6d1b3640222a0bd6a6182d3bd4db304132c7cb`.
Pre-existing edits to `carrier.rs`, `content.rs`, and `object-provider/src/main.rs`
were preserved. The source fingerprints at inspection were:

- `carrier.rs`: `de4aabd3c65204c437bb0e7f35942f9bae13766d9972f196ffbf0a9d088e851d`
- `content.rs`: `ae314e582884231ecbc931c7fab153523298613b31fcff0d2ae7c13e2f966340`

The inspected lockfile contains Iroh 1.0.2 and Tokio 1.53.1. Custody C's running
Kubo reports 0.42.0, arm64/Linux. Container behavior and stored receipts are live
reads of historical evidence; source/binary parity for every installed component
was not established. The next execution must capture artifact receipts before
claiming an installed fix. Local tracking showed one unpublished commit ahead
and zero behind; remote refs were not refreshed during this investigation.

## Corrected workflow and timing

The three measured publishes are **protected content, cover image, metadata**.
The earlier hand-off called the cover metadata and called the metadata listing.
Source in `api/gateway_provider_proxy.rs::publish_runtime_custody_creator_listing`
places the actual listing publication later, after terminal mint evidence, and
requests three replicas with live multi-peer proof. A completed mint with a cover
can therefore publish four distinct CIDs; a mint without a cover omits that leg.

| Object | CID prefix | IPFS operation / input bytes | Availability time | Result |
| --- | --- | --- | ---: | --- |
| Protected-content directory | `QmZWH…` | `add_directory`, 29,043 | 172,245 ms | `network_available`, 3 replicas |
| Cover image | `QmfA1…` | `add_bytes`, 1,030,856 | 1,322 ms | `network_available`, 2 replicas |
| Metadata directory | `QmWcE…` | `add_directory`, 5,428 | 79,876 ms | `carrier_announced`, 1 replica |

The three availability calls total **253,443 ms**. This is not the duration of
one `RuntimeMintStep`: the `availability_publish` step ends at 09:32:40.315 with
176,505 ms, before cover and metadata publication. Chain planning starts at
09:34:02.373. Chain confirmation is outside the measured delay.

The metadata manifest identifies `kind=directory` and six files: the two token
JSON files, `content.json`, `contract.json`, `manifest.json`, and `metadata.json`.
Their sizes sum to 5,428 bytes. It contains neither `publisher_did` nor
`object_did`. This directly establishes which artifact the slow third publish is.

Earlier measurements of protection (66 ms), custody provisioning (262 ms),
availability receipt (518 ms), manifest read (483 ms), and file verification
(953 ms) remain useful. Local IPFS add took 2,242 ms for the first directory,
61 ms for the cover, and 57 ms for metadata.

## Root cause 1 — remote pin-by-CID precedes transfer

**Confirmed for this run; dominant latency.**

`carrier.rs::ensure_content_via_carrier_provider_invocation` performs admission,
remote `content.ensure`, an import fallback if needed, and remote `content.status`.
The remote `content.rs::pin_for_availability` invokes IPFS `pin` with only the CID.
`ipfs-provider::pin` calls Kubo `/api/v0/pin/add` with a 300-second HTTP ceiling.
The publisher has the bytes, but it does not transfer them over Carrier before
this pin attempt. The recipient can spend minutes fetching the new DAG through
its independent Kubo network.

### Protected-content first candidate: custody C

| Event | Time / duration |
| --- | --- |
| Gateway topic ready / candidate starts | 09:29:46.058, approximately |
| Custody C sends IPFS `pin`, bridge lock wait 0 | 09:29:46.067 |
| Pin observer | CID remains in wantlist at 15, 45, 75, 105, 135, 165 seconds |
| Kubo pin succeeds | **168,042 ms** |
| Bridge pin response completes | 09:32:34.250, 168,186 ms |
| Custody C subsequently runs `add_directory` | 09:32:35.890 |
| Gateway candidate succeeds | 09:32:36.272, **170,217 ms** |

The request reaches the remote IPFS bridge about 9 ms after topic readiness.
Thus this interval cannot be attributed to a 170-second Carrier dial. Kubo pin
accounts for about 98.7% of that candidate's duration. The sampled Bitswap
counters stay unchanged while the target CID remains wanted; those counters
are daemon-wide, so they do not establish per-object byte throughput.

### Metadata third candidate: custody A

```text
09:32:46.880  candidate 1 failed   4,437 ms  (custody C)
09:32:51.938  candidate 2 failed   5,057 ms  (custody B)
09:32:51.947  custody A starts pin, bridge lock wait 0
              pin waiting: target wanted, zero received blocks at 15 and 45 s
              pin succeeds after 66,185 ms
09:33:58.383  custody A bridge pin completes
09:34:02.315  candidate 3 failed  70,378 ms  cumulative 79,873 ms
09:34:02.316  availability_ensure returns    total      79,876 ms
```

Candidate durations sum to 79,872 ms; rounding and local work explain the few
remaining milliseconds. The third candidate accounts for the entire alleged
post-loop gap. Custody C and B pinned this metadata in 57 and 59 ms respectively;
A took 66 seconds even though this is later in the same mint.

**Underlying network uncertainty.** Pin acquisition delay is proved; the exact
routing/discovery failure that delayed the blocks is not. Kubo peering already
exists in `up.sh`, `entrypoint.sh`, `server_infra.rs`, and `ipfs-provider`.
Custody C's startup log shows peering adds, and its config includes the host and
custody peers. Adding the same configuration again is not a demonstrated fix.
The host config has custody peer IDs with empty address lists. A later C swarm
snapshot did not show the configured custody/host peers; the host Kubo coordinate
file disappeared during inspection. Those later observations do not reconstruct
the historical swarm. Capture connected peer identities, resolved addresses,
listeners, and block flow together in the next controlled reproduction.

Carrier rendezvous and Kubo content routing are separate mechanisms. Fast Carrier
gossip subscription does not rule out Kubo block-discovery delay.

## Root cause 2 — availability request wire mismatch triggers needless imports

**Confirmed by custody repair-task records and source.**

After a successful local pin, `content.rs::pin_for_availability` always calls
`ensure_network_availability`, including the replica request with
`min_replicas=1`, `max_replicas=1`, and `require_live_multi_peer_proof=false`.
The standalone storage host registers the external `availability-provider`
through `provider_host.rs` and `server_infra.rs::register_availability_provider_plane`.

`ProviderRegistry::invoke_provider` attaches `_runtime_invocation` to the request.
The external provider's `Request` / `EnsureRequest` uses `deny_unknown_fields`
and has no field for that envelope. It also lacks the `accounting` and optional
`estimated_content_bytes` fields sent by the content adapter.

Custody C's stored repair tasks for both affected directories contain:

```text
unknown field `_runtime_invocation`, expected one of `cid`, `uri`, `policy`,
`local`, `requirements`, `object_did`, `publisher_did`
```

The successful pin becomes an availability outcome of `repair_needed`, replicas 1.
The gateway interprets that aggregate status as needing import. For protected
content, the redundant directory import succeeds and creates a second receipt
with `policy=carrier_object_import`, `status=local_pinned`. For metadata, the
fallback fails as described below. The cover succeeds through exact-byte import.

There is also a latent deployment problem: the inspected storage host uses
`ELASTOS_AVAILABILITY_ENSURE_URL=https://replica.invalid/ensure`, the Compose
default. The decode error occurs before HTTP. Merely accepting the envelope
would expose the invalid target next; it would not make this path healthy.

The contract needs two distinct facts: **this admitted peer stores this CID**,
and **the publisher has enough independent replicas**. A replica request capped
at one local copy should establish the first fact without requiring that peer
to contact an unrelated external placement service. The publisher retains
responsibility for the second fact and its signed evidence.

## Root cause 3 — fallback changes a metadata manifest and therefore its CID

**Confirmed source defect, consistent with all three installed failure sequences.**

1. Metadata is published with `object_did=None`, `publisher_did=None`.
2. `content.rs::publish` constructs `_elastos_object.json` from those original
   fields, then selects an effective publisher for accounting and replication.
3. Carrier receives that effective publisher in its source request.
4. `import_object_content_via_carrier_provider_invocation` uses
   `manifest.publisher_did.or_else(source_request.publisher_did)` and the same
   pattern for object identity.
5. The receiver regenerates the manifest with that added publisher. The manifest
   bytes and directory CID change. `content.rs::import_object` detects the CID
   mismatch and unpins the newly created, wrong-CID object.
6. Carrier then tries `import_exact`. Fetching a directory as bytes and adding it
   as one file does not reproduce the original directory DAG. That path also
   returns a mismatch and unpins its newly added CID.

The source manifest read from custody C has no identity fields. The gateway
receipt supplies its effective node publisher. On every custody node the log
shows `add_directory → unpin → add_bytes → unpin` during the failed metadata
candidate. These observations align with the mismatch branches in the source;
the candidate logger does not print the final error text, so per-call generated
CIDs were not recovered from that logger.

Keep original content identity separate from receipt/accounting identity.
Copying the exact manifest, or reproducing its optional fields exactly, is
required. A valid publisher for an accounting receipt does not authorize changing
the published bytes. Explicit-identity protected content happens to avoid this
case; identity-free directory publishes share the defect.

The metadata's `carrier_announced` / 1-replica result follows from its default
minimum of one and no required live proof. All three remote proofs failed.
It describes local availability with an announcement, not three proven copies.
It also does not mean all remote Kubo pins failed: their successful pins are in
the logs. The remote signed proof path is what failed.

## Amplifiers and adjacent correctness gaps

- **Sequential fan-out.** `announce_availability` walks up to eight candidates
  until enough proofs succeed. Each complete candidate has no total deadline.
  A five-second route timeout bounds each connect; the response read separately
  allows 360 seconds by default. Stream opening/writing and multiple calls or
  ticket endpoints add further waits. Parallelism alone would still wait for
  Kubo acquisition or reproduce invalid metadata on every peer.
- **Cancellation leaves work running.** `ProviderBridge::send_json_line` spawns
  a task that holds the provider I/O lock until it drains one response. This
  preserves response alignment after caller cancellation. A timed-out Carrier
  request can therefore leave a 300-second pin running and block a subsequent
  import to the same IPFS bridge. Preserve this safety property while bounding
  backend work and retries; do not solve latency by abandoning pipe responses.
- **Misleading failure telemetry.** `reached` is `invocation.is_ok()`, meaning
  complete proof success, not network reachability. Metadata logs say false
  even though the remote peer received the request and pinned the CID. Failed
  candidate reasons are collected but omitted from the response when the local
  minimum permits `carrier_announced`; reputation failures mix network errors
  with deterministic serialization errors.
- **Repair state hides desired shortfall.** The gateway metadata repair task is
  `healthy` despite three failed remote attempts. `record_repair_task` maps
  `carrier_announced` to healthy based on status alone. A later health check is
  scheduled, but normal due-work selection needs `include_healthy_check` to
  revisit it. Distinguish a satisfied minimum from an unmet desired replica
  count and record actionable failure reasons.
- **Multiple dependent objects.** Cover precedes metadata because metadata names
  its CID. The final listing names metadata/content and terminal mint evidence.
  Keep those bindings. A CID in a JSON field is not automatically an IPLD link
  that recursive pinning will traverse. One root pin cannot be assumed to cover
  this whole application graph.
- **Relay warnings remain separate evidence.** The endpoint-ID collision warning
  is at 09:35:08, after this latency interval. The inspected gateway identity and
  three custody identities are distinct. This excludes simple identity reuse
  among those four recorded identities, but not another process using one key.
  Authentication warnings also occur during successful attempts. Neither warning
  explains the measured remote pin intervals. Investigate process ownership if
  warnings persist; preserve identities and custody state.

## Ordered fix plan

The following work is proposed, not implemented. Preserve quorum, admission,
CID verification, signed receipts, custody policy, journal recovery, and existing
content. Use the current shared functions rather than Creator-specific bypasses.

### 1. Add failing contract and CID regressions

Affected surfaces: `content.rs`, `carrier.rs`, `provider/registry.rs`, and the
external `availability-provider` test harness.

- Exercise a real Runtime-enveloped content-to-availability request through the
  external provider parser, including accounting fields. Existing direct-request
  provider tests do not cover this composition.
- Publish/import an identity-free directory while the replication source carries
  an effective publisher; require identical manifest bytes and root CID.
  Cover explicit identities, links, multiple files, and nested paths as well.
- Reproduce successful local pin followed by an unavailable external placement
  provider. A one-copy replica request must still yield a truthful local proof;
  a publisher request for three live copies must retain that stronger gate.

**Gate:** failures demonstrate the observed defects before implementation.

### 2. Repair local-replica semantics and the provider boundary

Affected surfaces: `content.rs::pin_for_availability`, external availability
provider decoding/adapter, and standalone provider composition tests.

- Give the admitted replica operation an explicit local placement/proof path.
  Verify the exact CID and local pin, write the signed local receipt, and let
  the publishing Runtime count distinct verified peers.
- Keep ordinary network ensure/repair semantics for callers requesting network
  placement. Specify the local-only condition explicitly rather than treating
  every provider error as a successful local copy.
- Align the external provider's input with the Runtime envelope and accounting
  contract. Parse/validate Runtime metadata separately from the outbound target
  payload; retain strict rejection of unknown authority fields. Do not globally
  strip envelopes or make the HTTP target trust caller-supplied Runtime fields.
- Remove the fake endpoint dependency from the local replica role. If external
  placement is required for a deployment, require a real configured target and
  test it separately.

**Gate:** successful pin yields one valid local proof without redundant import;
external placement errors remain visible for callers that require it.

### 3. Preserve the exact published object during import

Affected surfaces: `carrier.rs::import_object_content_via_carrier_provider_invocation`
and `content.rs::import_object` / manifest construction.

- Preserve the source manifest's optional identity fields exactly. Carry any
  effective publisher needed for admission/accounting separately. Prefer the
  smallest change that keeps these two uses distinct.
- Retain size, digest, path, file-count, and exact root-CID checks.
- Select fallback by actual representation: directory/object import for these
  manifests; exact bytes for compatible single-file objects. Avoid knowingly
  trying exact-file import after a directory identity mismatch.
- For arbitrary DAGs, reuse the existing block-graph export/import capability
  after proving installed support and limits. The custody readiness inspected
  here lists custody, availability, IPFS, and chain providers, not block-graph.
  A repository implementation alone does not make that route deployable.

**Gate:** all three metadata peers can provide matching-CID receipts, with no
`add_directory → unpin → add_bytes → unpin` sequence for valid inputs.

### 4. Remove speculative remote fetch from fresh placement

Affected surfaces: Carrier replication orchestration and local content/import
operations; IPFS provider only where an explicit backend budget is needed.

- After admission, use a bounded local-presence check. For a missing fresh CID,
  transfer the source object/bytes through the existing authenticated Carrier
  import route, then verify the exact CID and pinned state.
- Make the local-presence check genuinely local. Current `status` is a stored
  receipt view; it does not by itself prove current block presence. Avoid a
  supposedly cheap check that starts another network pin or fetch.
- Retain existing size/graph limits. Define a separate bounded path for larger
  media and arbitrary DAGs; avoid forcing them through small JSON/base64 limits.
- If retaining a pin-first optimization, give it a small explicit backend budget
  and prove it releases the bridge before fallback. Lowering only the Carrier
  response timeout does not cancel the remote pin.

**Gate:** a fresh small object replicates when custody has no Kubo route to the
publisher but Carrier is reachable. Repeat from cold and warm processes. This
isolates the fix from public IPFS discovery timing.

### 5. Bound fan-out and improve evidence

Affected surfaces: `carrier.rs::announce_availability`, Carrier invocation timing,
content receipts/repair tasks, and progress attribution.

- Use bounded concurrency, initially two candidates, retaining the eight-attempt
  guard and distinct-peer counting. Launch replacements when attempts fail;
  preserve admission and quota checks. Account for concurrent placements before
  claiming that `max_replicas` is a hard cap.
- Apply an absolute candidate and overall placement deadline, covering connection,
  stream open/write, remote work, local import preparation, and final proof.
  Carry remaining time across subcalls rather than resetting the budget each time.
- Return an explicit incomplete/repairable result at the deadline. Preserve journal
  state and reconcile late remote completion on retry. Avoid duplicate work to
  a peer that is still processing the previous request.
- Log CID, workflow object role, candidate identity, operation, lock wait, connect,
  response wait, import strategy, outcome/error category, and remaining budget.
  Rename `reached` to `proof_ok`, and record reachability independently. Keep
  private routes, tickets, keys, and content out of user-facing responses.
- Keep transient network failures separate from contract/CID errors in reputation.
  Preserve failed-candidate reasons even when the minimum policy is satisfied.
  Represent desired-replica shortfall in repair state and test its scheduler.

Proposed initial acceptance target for the local small-file fixture: all required
pre-wallet publication work within 15 seconds in healthy cold and warm runs;
a required placement failure returns a resumable result within a 30-second
workflow budget. These are proposed targets, not measured achievements or
universal large-media limits. Set final budgets from the corrected-path timings.

**Gate:** one silent/dead/slow peer cannot prevent healthy candidates from running;
expiry leaves bounded work and correct receipts, and subsequent operations remain
responsive. Fake-time tests should verify the deadline, not merely its constant.

### 6. Installed acceptance and only then optional optimization

- Bind source tree, binaries, provider manifests, and capsule artifacts to one
  verification run; collect logs from gateway plus all three custody nodes.
- Run cold/warm, cover/no-cover, small document/media, dead first peer, stalled
  pin, invalid envelope, CID mismatch, restart/resume, and concurrent publish cases.
- Verify content, cover, metadata, and final listing separately. Confirm receipts,
  required replica counts, exact CIDs, and buyer fetch/open with the publisher
  unavailable. A fast publish response alone is insufficient.
- Use an isolated local/forked chain and fresh fixtures for transaction tests.
  Preserve real mint journals and custody state. Resume an existing mint through
  its recovery path; do not delete its journal to force a second mainnet mint.
- Apply the journey-register acceptance process before declaring product readiness.
  Keep source, simulated, installed, and human Creator evidence distinct.
- If transport remains material after these fixes, evaluate endpoint reuse and
  readiness/warm-up. Existing Kubo warm-up and peering code should be verified
  before adding more startup work. Consider reuse of verified export bytes across
  candidates only after timings show it matters.

## Evidence commands for the next investigator

Read-only commands; use the full interval and all candidate attempts:

```sh
rg -n 'content publish phase|candidate settled|runtime custody phase' .vscode/logs/gateway.log
for node in a b c; do
  docker logs --since 2026-09-18T09:29:00Z --until 2026-09-18T09:34:10Z \
    "custody-host-custody-${node}-1" 2>&1 |
    rg 'pin waiting|pin settled|provider request.*(pin|ensure|add_directory|add_bytes|unpin)'
done
```

Receipts and repair tasks are under `ElastOS/SystemServices/Content/` in each
Runtime data root. Inspect `availability-receipts.jsonl` and `repair-tasks.jsonl`
by exact CID, selecting only the needed fields. Check local Kubo API coordinates
at read time; daemons can stop when idle. Full CIDs for this record:

```text
content   QmZWHHbKrQnsJ8N9bHrSJFLwgQ1c2SMLFCeRjs5M7ZKsLY
cover     QmfA1GtjaHjStnaz9KpN4r9Boz6LKaZKBuh5zgjPAJ6HGF
metadata  QmWcE4pFS8k6XijFoR8j81A1CoKKm6iD88fVgXKkSbP7yi
```

Required diagnostics include `elastos_server=debug`, Carrier trace, and provider
bridge operation timings on the custody nodes. Gateway-only pin logs miss the
principal cost. Existing `pin waiting` instrumentation did fire in this run.

Primary references checked on the investigation date: [Tokio 1.53.1 timeout
semantics](https://docs.rs/tokio/1.53.1/tokio/time/fn.timeout.html) establish future
cancellation, not cancellation of a remote side effect; the detached bridge task
behavior above comes from repository source. [Kubo RPC documentation](https://docs.ipfs.tech/reference/kubo/rpc/)
describes pin and DAG operations; its current page is generated for a newer Kubo
than the installed 0.42.0, so validate any new RPC options against the installed
version before implementation.

## Remaining limits and separate defects

The exact cause of delayed Kubo block delivery and the owner of the relay identity
collision remain open. Neither prevents fixing the confirmed serial pin-first,
wire-contract, and CID-preservation defects. Historical runs on 2026-09-17 need
the same cross-node correlation before assigning them the identical cause.

The existing object-provider BrokenPipe handling edit is separate and preserved.
Similar unchecked stdout writes remain a follow-up for other provider processes.
Creator stage polling already exists; improve object-role attribution rather
than using progress text as a substitute for fixing latency.

The requested humanizer skill was absent from the searched project and installed
skill directories. The report was manually reviewed for direct wording and
explicit evidence limits. No independent agent review or installed fix acceptance
is claimed.

## Verification of this documentation update

- `git diff --check`: passed for tracked working-tree edits.
- `node scripts/home-entropy-check.mjs`: passed, including Markdown link checks.
- `cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check`: passed.
- Workspace `cargo fmt --all -- --check`: failed on two pre-existing formatting
  differences in the instrumentation: the split `match invocation` brace in
  `carrier.rs` and the `repair_task_ms` expression in `content.rs`. Both are left
  unchanged. This documentation task does not claim a green source gate.
- No implementation tests or fresh mint were run. Behavioral claims above are
  limited to inspected source and correlated existing logs/receipts.

Next action: implement the failing composition and identity-free-directory tests
in plan step 1, then repair local replica proof semantics and exact import before
measuring latency again. All implementation steps remain pending.

## Known issue — a settled effect recovered from backup never reconciles

Observed 2026-09-18, 13:19–13:24, not diagnosed.

A mint transaction was broadcast and **succeeded on chain** (Base block
51467896, tx `0x263f13c8c5615abda874b14db894eccc0ef798f66f00c8ad6fcf0d9dcee44709`,
status 1, 25 logs). The gateway exited mid-reconcile before the receipt was
recorded, because its own binary changed on disk. The per-principal
`transaction-effects.json` was then deleted and later restored from a backup.

On retry the creator tail:

- resolves a chain plan and ensures the **same** effect
  (`transaction-effect:sha256:9c85ad9de…3244`) rather than creating a new one,
- reports `wallet_approval outcome="ok"`, raises **no** new approval
  (`awaits_person=false`) — so it is not attempting a second mint,
- returns `pending … reason="chain_settlement"` at
  `gateway_provider_proxy.rs:4222` every ~6 s indefinitely.

Across three minutes of polling there was **not one chain transaction query** —
only `describe_protected_content_creator_mint_source` and
`resolve_protected_content_creator_mint`. `reconcile_transaction_effect` is
gated at `gateway_transaction_effects.rs:1089` on `receipt.is_none()` and
`state ∈ {BroadcastInFlight, Indeterminate}`; one of those is presumably false,
but the effect store is an encrypted principal-root envelope and its state was
not read. The mint id is new (`925b6f07…`) while the effect id is the original,
so a fresh draft adopted the pre-existing effect.

The store is live during the loop: the active principal's effects file was
rewritten each cycle and grew from 430,743 to 467,142 bytes in about four
minutes. Whether that is audit append or unbounded growth is not established.

To diagnose: log the effect state and the `completion` fields in the pending
branch, then reproduce once. Note that rebuilding the host binary while the
gateway runs causes it to exit ("host binary … changed on disk"), so stage the
change and build with the gateway stopped.


## Elacity public metadata requirement (2026-09-21)

The user identified the Elacity IPFS node behind `https://ipfs.ela.city` as a
required public metadata destination:

```text
/ip4/34.77.31.164/tcp/4001/p2p/12D3KooWNieM3HRBJdVqaQucZEJdqA3oWKrKf3Gx3hp2cmtR9GNK
/ip4/34.77.31.164/udp/4001/quic-v1/p2p/12D3KooWNieM3HRBJdVqaQucZEJdqA3oWKrKf3Gx3hp2cmtR9GNK
```

The IPFS provider source now includes both addresses in its default persistent
peering configuration and adds them to the existing bootstrap set before starting
Kubo. Operator-configured peers remain. This prepares connections for new managed
instances; deployment and live connectivity verification are still pending.

The automatic Elacity peer is **enabled by default**. Leave
`ELASTOS_IPFS_ELACITY_PEER_ENABLED` unset, or set it to `true`, until the user
instructs otherwise. To retire the automatic integration later, set this variable
to `false` in the environment that launches Runtime/the IPFS provider, then
restart the provider and its managed Kubo daemon. Values other than `true` or
`false` are rejected as configuration errors.

At the next managed daemon start, disabling removes the two automatic bootstrap
addresses and omits the default peer from persisted peering. Kubo's `auto` entry
and unrelated bootstrap peers remain. Explicit operator peering entries remain
operator-owned: remove an explicit entry for this same peer too if retiring it
entirely. Adopting an already running daemon does not apply retirement; restart
Kubo for that transition. The switch controls connectivity only, not the future
public-metadata placement policy or files already pinned on the Elacity node.

Bootstrap and peering establish connectivity. They do not send a pin request to
Elacity or prove that its gateway retains an object. The metadata publication
acceptance gate must separately establish durable placement on that node and
verify anonymous reads through its public gateway:

- Pin the exact metadata directory CID through an authorized storage operation.
  Obtain a placement acknowledgment; public gateway reads alone may only populate
  a temporary cache. Determine the node's supported pin/admission interface before
  implementation. Its libp2p ID does not establish a Runtime Carrier interface.
- Fetch every file listed in `_elastos_object.json` through
  `https://ipfs.ela.city/ipfs/<metadata-cid>/<path>`, including `content.json`,
  `metadata.json`, `contract.json`, `manifest.json`, and all token JSON files
  such as `0000000000000000000000000000000000000000000000000000000000000001.json`.
  Verify successful responses, expected bytes/digests, and retrieval latency.
- Verify referenced cover images separately. Repeat retrieval with the publisher
  offline and with a fresh reader so publisher availability and browser caching
  do not hide missing placement.
- Expose pending public metadata placement as a resumable publication state rather
  than claiming public availability from a local pin or peer connection.

Metadata and cover availability have priority for discovery/indexing. Encrypted
media replication retains the existing minimum availability and custody policy;
any deferred payload placement is a separate policy decision. This addition does
not implement a remote pin service, change publication gates, or claim that the
historical metadata CID is currently retained by Elacity.

Verification of the startup addition: the installed Kubo 0.40.1 CLI accepted
both bootstrap addresses and the peering configuration in a temporary isolated
repository. Repeated bootstrap additions preserved existing entries and added no
duplicates; the temporary repository was removed. Provider formatting, chain
provider formatting, whitespace, and Home entropy checks passed. Regression cases
were added for default peering and merging configured addresses. Compilation and
Rust test execution are pending because free disk space is below the repository's
10% build threshold. The pre-existing workspace formatting differences remain.
No provider rebuild, installation, restart, or live gateway proof was performed.

The retirement switch was also checked with installed Kubo 0.40.1 in an isolated
repository through enable, repeated enable, disable, repeated disable, and
re-enable. The final configuration path preserves `auto` and unrelated bootstrap
entries. It uses `config --json Bootstrap`: `bootstrap rm` was experimentally
rejected by this Kubo when `auto` was present. Regression cases cover default-on,
disabled automatic peering, invalid settings, and bootstrap-list preservation.
Rust test execution remains pending under the disk-space gate above.

## Second measured run — 2026-09-21, with stage instrumentation in place

This run repeats the 2026-09-18 measurement on the same macOS host and the same
three local custody containers, with the publish-stage timers and the
working-tree candidate timers active. It confirms root causes 1, 2, and 3, and
it settles the open question about the DHT topic join. The run made no source
change and no service restart. The creator was configured for Base mainnet.

### Evidence and scope

The recorded interval is 13:01:12–13:19:13 UTC on 2026-09-21.
Request id `9623bb93…`, mint id `c75c881d…`. Container names are
`custody-host-custody-{a,b,c}-1`. The exact source commit and working-tree state
for the run are recorded with the evidence snapshot named below.
The evidence is snapshotted, with a `SHA256SUMS` manifest, in the untracked
directory `.vscode/logs/mint-20260921/`: the gateway slice for this mint, all
three container logs, and all three repair-task journals. Source fingerprints at
inspection were `carrier.rs` `e6e48e8d…`, `content.rs` `ae314e58…`, and
`ipfs-provider/src/main.rs` `ee6e8e5e…`.

Node identities for this run:

| Container | Carrier DID | Kubo peer id |
| --- | --- | --- |
| custody-a | `did:key:z6MkpuCfBDqoQ…` | `12D3KooWQxLQaVHrj9zYUj4Bwbsm4uU39Vc1vdSV19a1xPTc7Vjp` |
| custody-b | `did:key:z6Mkkboq…` | `12D3KooWLDwsHUY52fMieXSdkw4UVvgPdrt5vR4CJntiMRoTSakS` |
| custody-c | `did:key:z6MkfVNErQ3v…` | `12D3KooWQGP6TDvHKE7ME52P6pJB9ErbAXErMQoTRdP9cZ9ME483` |

### Measured timeline

From the start of `protect` to the resolved chain plan the mint took
**1,012.5 s (16 min 52 s)**. This publish carried no cover image, so it has two
publishes rather than three.

| Step | Duration | Note |
| --- | ---: | --- |
| `protect`, 5,322,431 framed bytes in 6 chunks | 2,440 ms | |
| `rights_policy` | 0 ms | |
| `custody_provision` | 336 ms | |
| `staging` | 1 ms | |
| **`availability_publish`, protected content** | **354,758 ms** | `QmVaZQ…` |
| ├ `ipfs_add` `add_directory`, 5,322,681 bytes | 179 ms | |
| ├ gossip topic join | 1 ms | |
| ├ **`availability_ensure`** | **352,502 ms** | `network_available`, 3 replicas |
| ├ receipt and repair task | 55 ms | |
| ├ `availability_receipt` | 512 ms | |
| ├ `object_manifest` | 471 ms | |
| └ `object_files_verify` | 999 ms | |
| **metadata publish** | **654,843 ms** | `QmVCay…`, 4,980 bytes |
| ├ `ipfs_add` `add_directory` | 44 ms | |
| ├ **`availability_ensure`** | **654,714 ms** | `carrier_announced`, 1 replica |
| └ receipt and repair task | 49 ms | |
| `chain_plan` | 1 ms | Base mainnet, awaits the person |

Remote replication accounts for **1,007,216 ms, or 99.5%** of the publish. The
local and cryptographic work totals about **5 s**.

Per candidate:

| CID | Attempt | Node | Duration | Gateway verdict |
| --- | ---: | --- | ---: | --- |
| `QmVaZQ…` | 1 | custody-c | 20,712 ms | reached |
| `QmVaZQ…` | 2 | custody-b | **331,782 ms** | reached |
| `QmVCay…` | 1 | custody-c | 4,579 ms | failed |
| `QmVCay…` | 2 | custody-b | **322,864 ms** | failed |
| `QmVCay…` | 3 | custody-a | **327,267 ms** | failed |

### The DHT topic join is settled and carries no cost

The announce and join timers exist to answer whether a gossip topic join
explains the earlier unattributed time. It answers: on both publishes the
join reported `lock_wait_ms=0`, `subscribe_ms=0`, `split_ms=0`, and
`join_ms` of 1 and 0. The announce lock and the join are each immeasurable.
Treat this hypothesis as closed and keep the timers as a regression guard.

### Root cause 2 reproduces on every node

Each container wrote a `carrier_replica` repair task for this run with the
reason `unknown field \`_runtime_invocation\`, expected one of \`cid\`, \`uri\`,
\`policy\`, \`local\`, \`requirements\`, \`object_did\`, \`publisher_did\``.
The envelope rejection therefore still converts a successful remote pin into
`repair_needed` and sends the gateway into the import fallback. Custody C's
journal holds the pair for `QmVaZQ…` directly: a queued `carrier_replica` task
with that reason, followed by a `carrier_object_import` task at `local_only`.
Its Kubo pin had already succeeded in 15,670 ms. That pin bought nothing.

### Root cause 1 reproduces, and the fallback measures its own remedy

Custody B held both CIDs in its wantlist for the full 300-second Kubo ceiling
and reported `blocks_received=0 data_received=0` at every 30-second probe, with
35 to 203 swarm peers present. Both pins ended as
`outcome=error … timed out reading response`, at 327,149 ms and 318,485 ms.

The protected-content candidate on that node still returned reached, because the
exact-byte import fallback then placed the object. The gateway settled the
candidate 2.5 s after the pin error, and custody B now answers
`pin ls --type=recursive QmVaZQ…` with `QmVaZQ… recursive`. So the transfer that
the caller already had the bytes for takes about **2.5 s**, and the pin-by-CID
attempt in front of it takes **300 s**. This quantifies plan step 4: removing the
speculative remote fetch from fresh placement recovers essentially the whole
delay, and the import path that remains is already proven on this host.

`ensure_content_via_carrier_provider_invocation` sets `timeout_ms: Some(5_000)`
on the Carrier route. That bounds the dial. The remote operation is bounded only
by the IPFS provider's own 300,000 ms pin ceiling, and the candidate loop carries
no overall deadline, so the cost is candidates × 300 s. The metadata publish paid
it three times.

### Root cause 3 keeps the metadata unreplicated

Custody C pinned the metadata directory in **51 ms** and the gateway still
recorded that candidate as failed after 4,579 ms. All three candidates failed and
`availability_ensure` returned `carrier_announced` with **1 replica**: the
creator's own node. The node that holds the bytes cannot be credited while the
import regenerates the manifest with an effective publisher and changes the
directory CID. Identity-free directories remain the exposed case.

### New amplifier — the managed Kubo idle watcher

All three custody Kubo daemons stopped at 12:54:20 after 600 s idle and then cold
started on demand. Warmth at request time predicted the outcome:

| Node | Kubo start | Relation to its request | Result |
| --- | --- | --- | --- |
| custody-c | 13:02:08 | 20 s ahead | pin ok, 15,670 ms |
| custody-b | 13:02:28 | at the request | 0 blocks, timed out |
| custody-a | 13:13:30 | at the request | 0 blocks, timed out |

A cold daemon starts with an empty peer table, and its peering dial to the
creator's Kubo through `host.docker.internal` is still in flight when the pin
request arrives. Object size did not predict pin latency in this run: custody B
spent the same five minutes on 4,980 bytes as on 5.3 MB. Raise the idle threshold
on custody nodes, or warm the daemon before admitting a placement, so that this
variable stops dominating the measurement of the real fixes.

### Consistency note on the protected-content receipt

The protected-content publish reported `network_available` with 3 replicas. Two
of those three placements came from the import fallback rather than from the pin
the receipt describes, and custody B still carries a queued `local_ensure_failed`
task for the same CID. The replica count is accurate; the node's own journal and
the publisher's receipt describe that placement differently. Reconcile the two
when plan step 2 repairs local-replica semantics.

### What this run changes in the plan

The ordered fix plan stands. This run adds measured weight to its steps:
close the envelope mismatch first, because it is a small contract change that
stops discarding pins that already succeeded; then remove the speculative remote
fetch, because 300 s per candidate is the whole delay and the 2.5 s replacement
is already exercised on this host; then bound and parallelise the candidate loop;
then preserve the exact manifest so metadata can reach more than one replica.
Raise the custody idle threshold alongside them so the next measurement is not
dominated by daemon warmth.
