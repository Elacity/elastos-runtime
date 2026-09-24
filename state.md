# State

## Current model foundation, 24 September 2026 UTC

Rows below retain dated diagnostic history. The latest public row supersedes
older statements about which Runtime or engine is currently installed.

| Surface | Receipt and exact identity | Proven behavior | Open boundary |
| --- | --- | --- | --- |
| Current isolated model source | `feat/0.7.1-model-assistant-closeout` commit `bb65974b` (tree `b8cd6199`); private `.audit/j3-shared-local-exact-revision-reply.private.json` SHA-256 `047d0168`. Both isolated Mac Homes have built, installed and running Runtime SHA-256 `a7114f39e02bf7442f8abee1d98b72dc75d7fad649b6434a1dab7ee245d1eff7`; installed and served Home hashes match. | The gateway now accepts the provider's canonical `sha256:` execution revision in the signed catalogue, exact request, grant and dispatch. The regression failed before repair and passed after it; 36 remote-model, 19 Services, 11 model-service and 8 model-remote tests, Clippy, format, diff and Home entropy passed. Independent review found no source blocker. | This source was installed only on the isolated pair. Public Runtime remains the separate reviewed Content projection below. The full J3 receiver, authority and recovery matrix remains open. |
| Isolated Owner/Consumer Homes 61986/61987 | Same source and Runtime hash above; separate owner-only identities and data roots. Consumer's ordinary signed SmolLM2 Get admitted 144,835,448 complete package bytes and a 144,811,072-byte GGUF with SHA-256 `c4a3dd03`. The private exact-reply receipt binds one Owner and one Consumer run record to the same grant and request. | Consumer Services received Owner's exact named SmolLM2 catalogue. The person approved its six-hour grant in Owner Inbox. Consumer Marketplace listed the exact shared offer, Assistant selected the remote Owner route, and one short prompt completed at Owner with a terminal reply visible in Consumer Home. Both Homes retained the grant, reply and installed model through restart. After Owner restart, one Consumer discovery used a stale isolated publisher ticket; refreshing that peer ticket and restarting Consumer restored the exact listing without another run. | The Consumer Get used the seed route, so it does not prove delivery from a separate holder. No second dispatch after Owner restart was attempted. End/refusal, second-principal denial, failed transfer, retry, recovery and sustained inference remain open. The stale publisher ticket was isolated route configuration, not a grant revision change. |
| Isolated exact grant End, 24 September | Same `bb65974b` source and `a7114f39` installed Runtime; private `.audit/j3-shared-local-exact-end-refusal.private.json` SHA-256 `c8962725`. | Owner used Inbox **Revoke access** on the one active exact SmolLM2 grant. Its active row disappeared while the older generic and Wallet requests remained. Consumer Services showed **Denied** after refresh, and Marketplace then showed no shared model. One attempt through the stale selected remote offer produced “Run acceptance is unknown”; Owner service runs and model-provider journal stayed at one prior completed run with no new pending entry or provider journal change. After an Owner restart, the End state and Consumer denial remained, and installed/served hashes and Home health matched. | This proves zero new Owner dispatch for the bounded attempt and removal of Consumer access. It does not prove an authenticated pre-dispatch refusal: the saved run ticket addressed the earlier Owner process, and Assistant reported unknown acceptance. The earlier successful reply and protected request state remain; a separate immutable history UI was not observed. Fresh holder delivery, second-principal denial and failure/retry/recovery stay open. |
| Earlier local source candidate | `feat/0.7.1-model-assistant-closeout` code baseline `7f4e27de` (tree `8c84e039`); Smol readiness code `a1573675`; CID-scoped receipt repair and three regression tests `e06950d5`. | At that checkpoint, the branch contained the repaired Content lookup and focused tests. The human Owner reply on port 61965 remained bound to the older `a1573675` artifact; the public Runtime was a separate one-function projection of the repair. | Superseded by the current isolated source row for the two-Home test. The human and public installations remain separate evidence. |
| Owner Home 61965 | `.audit/local-smol-readiness-installed.json`; built and installed Runtime SHA-256 `c4c6286573a23cd06a1727ef29434386d15ffec1c42cbcc90673dad4417abf3c` from `a1573675` | Signed-in System shows retained SmolLM2 available; ordinary Assistant selected its exact offer and completed a new reply. | This Owner installation does not qualify Consumer, public, full Qwen or the wider failure matrix. |
| Consumer Home 61966 | `.audit/codex-hosted-egress-containment-installed.json`; Runtime SHA-256 `90edb7fa1474d23ded52abee44f87c31a4f45f24eb083a8bef658ac6ef84176c` | Signed-in Consumer remains available; hosted Validate/Save pause was installed and checked with a controlled endpoint. | No SmolLM2 admission here; hosted HTTPS and the separate service-grant journey remain open. |
| Former public Runtime and provider, before repair | `f087a0d9` (tree `87e61b3e`); `.audit/public-seed-gate-61974/receipt.json` and private `public-ledger-repair-cause.private.json`; Runtime SHA-256 `62ddda9a31b9b512ebb82deb6c8152229b1c84dee969cf8621e2c742755a1d70`, ipfs-provider SHA-256 `0609b6e089e8ab08254f41c4f685a39c6c7d5c8bc8cfe24a6b1457705df3152e` | At that observation, the public Runtime/provider matched installed hashes. Public Home 8090 and holder 61954 use separate data roots; both Kubo repositories now return the same 726-byte index for the signed SmolLM2 CID offline. The public availability ledger SHA-256 `0f6b0b0e` has 27 rows, 24 without `peer_selection`, `quota` and `repair_worker`. | Six Marketplace and one System Get records failed at `metadata_read`; a supplied read-only public probe reported `content receipt ledger decode failed: missing field peer_selection` after local index bytes were read. Its source/artifact identity was not retained. Independent ledger census, source trace and a signed synthetic-row regression support a product cause: CID-specific fetch decoded unrelated old rows before returning bytes. That original Runtime was later replaced by the narrow repaired build in the public Get row below. |
| Bounded index diagnostic candidate | `43d8bd21` (tree `fcba5c4a`); private `index-substage-diagnostic.json` and `local-only-carrier-index-probe.json`; isolated Linux source snapshot from `f087a0d9` plus patch SHA-256 `5c379d12`; Linux test binary SHA-256 `7c1e92c5`; installed isolate receipt `a07f82e9` | A controlled 64 KiB index failure on Mac and isolated Linux source emits `local_bounded_index_cat` and `availability_fallback=failed` without CID/private marker or false Runtime-validation stage. Built, installed and running diagnostic Runtime SHA-256 `9363e82e` match on isolated Home 61974; its Home returns HTTP 200 and admitted SmolLM2 bytes remain unchanged. One exact user-approved incremental-cache removal restored Linux free space to 34,800,832/314,748,412 KiB (11.06%) after installation. One later local-only, 763-byte synthetic Carrier request reached that installed gateway and emitted `local_bounded_index_cat`; final free space was 32,037,580/314,748,412 KiB (10.18%). | The installed probe covers the Carrier-to-Content local bounded read only. It did not exercise `prepare()`, outer Runtime stream validation or the public Get failure. The newer receipt-ledger diagnosis supersedes this probe as the cause investigation. At that diagnostic observation, public Runtime was `62ddda9a`; the later reviewed repair and public Get supersede this open gate. The earlier isolated install receipt describes the prior binary and is superseded by the diagnostic install receipt. |
| Public UI and manifest overlay | `81875544` (tree `a0cfeb3e`) over `f087a0d9`; `.audit/hosted-egress-design-scratch/installed-ui-33aba416/public-live-postcheck-81875544.json`; manifest SHA-256 `4dd3f1e42a71b0474a6e34264048a4bab88f52945bf8fb2610f00060763ccd27` | Installed and served Home/System assets match the overlay receipt; signed-in System shows hosted HTTPS paused. | UI and manifest identity differs from the public Runtime source; a UI check is not public model Get or inference. |
| Superseded diagnostic-only public candidate | Local `0af13144` review commit; public f087 source basis plus Content patch SHA-256 `3592db2a`, derived tree `beb9cfe5`; private `diagnostic-candidate-signed-get.private.json` and `public-diagnostic-candidate-0af13144.private.md` | Linux built, one-link installed and running diagnostic Runtime SHA-256 `da9b9945` match. A distinct isolated signed SmolLM2 Get admitted 726 index bytes and 144,835,448 package bytes; installed GGUF SHA-256 `c4a3dd03` matches the prior isolated receipt. | The diagnostic-only public swap is superseded by the receipt-decoding finding; it has no public approval and was not installed publicly. Its isolated Get remains valid off-public evidence. At that review, public Home had no llama.cpp engine bundle. The later separately approved engine installation and reply are recorded below. |
| CID-scoped Content receipt source repair | `e06950d5`; private `public-ledger-repair-cause.private.json` | A genuinely signed old-format row before the requested CID made the unmodified bounded-index fetch fail with `missing field peer_selection`; the repair returns exact bytes for the requested current signed row. A matching tampered row and matching old row remain errors; whole-ledger dashboard status reports the incompatible ledger. Seventeen fetch, seven status, seventeen publish and one unpublish tests, formatting, diff and Home entropy passed. Independent source review found no blocking issue. | The repair has now passed a separate exact public-base Linux installed Get in the next row. The later public projection is recorded below. No signed ledger row was defaulted, rewritten or reissued. |
| Isolated public-base receipt repair Get | Public base `f087a0d9` (tree `87e61b3e`) plus only the reviewed CID lookup function from `e06950d5`; derived tree `e0641468`; private `public-f087-repair-isolated-get.private.json` SHA-256 `c2049fd3`; all 1,694 regular source files matched the derived-tree archive. Built, installed and running Linux Runtime SHA-256 `1ffafe36ebaa5b245c6b2a3e849157765ab31bac33f7e42f44459cc09dd39a16`; public-matching components SHA-256 `4dd3f1e4` and signed catalogue SHA-256 `c81d4357`. | Fresh isolated Home 61983 with separate generated identity and one synthetic, independently verified signed old-format receipt for an unrelated CID completed ordinary Marketplace `content.use` in 115.7 s. The operation journal records `admitted`, 726 index bytes and 144,835,448 completed package bytes. Installed GGUF is 144,811,072 bytes with SHA-256 `c4a3dd03`; the old row SHA-256 `62bca331` stayed unchanged. Fresh local Kubo bounded index cat failed, while the package completed through the source availability fallback; current repaired build has no diagnostic route tags, so this Carrier route conclusion uses source plus local-miss/completion evidence. Free space remained 11.86% at terminal. Earlier Home 61981 reached `metadata_read` with zero bytes because the explicit isolated Kubo path was missing; its uncertain state and receipt are preserved, and its gateway is stopped. Bounded independent review found no actionable P1/P2 in the one-function projection, source manifest or supplied installed evidence; it did not independently inspect the remote records. The minimal public projection omits the three feature-branch regression tests. | This remains isolated Linux evidence. The separate public signed-in Get passed in the next row. The later approved engine installation, bound reply, restart and observed zero-payload reopen are recorded below. The earlier diagnostic-only public candidate is superseded. |
| Public signed-in SmolLM2 Get on reviewed repair | Public source base `f087a0d9`, repair tree `e0641468`; private `public-repair-get.private.json` SHA-256 `100206bcc38cc3ca7f55a02674e93cd9090eaf158dc4f6498214892ef9445bcb`; reviewed candidate packet `public-repair-candidate-e064.private.md` SHA-256 `96408c2d`. Public installed/running Runtime SHA-256 `1ffafe36ebaa5b245c6b2a3e849157765ab31bac33f7e42f44459cc09dd39a16`; original SHA-256 `62ddda9a` is in a verified narrow rollback receipt SHA-256 `f0840117`. Components, signed catalogue, sources, provider and separate holder hashes stayed unchanged. | Fresh public preflight verified distinct public/holder identities, Ed25519-signed catalogue and pinned head, both recursive pins, 726-byte index, CPU/RAM/disk and local/HTTPS Home 200. One signed-in Marketplace Retry after a Home token refresh created one new `content.use` operation and reached `admitted` in about 139 s: 726 index bytes, 144,835,448 completed package bytes, one-link installed GGUF 144,811,072 bytes SHA-256 `c4a3dd03`, and matching signed catalogue CID/head. Marketplace and System agree the model is available; System shows Keep checked. Free space 11.68% at terminal. Runtime stale-host guard exited the old process after binary replacement; the reviewed candidate restarted with matching installed/running hashes and the served Home hash was unchanged. | The browser’s first expired-token click created no preparation record. The local Kubo and holder pin were present, but this repaired binary lacks route tags, so the actual Content/Carrier package route is inferred, not observed. The later engine and reply proof is in the next row. Wider J3 criteria and sustained seed inference stay open. |
| Public signed-in local reply and restart | Corrected approved engine packet `public-engine-candidate-b10516.private.md` SHA-256 `875b1871`; installed b10516 receipt SHA-256 `e89901ff`, `llama-server` SHA-256 `fa24fc90`, 52 files and ten internal symlinks; Runtime remains built/installed/running SHA-256 `1ffafe36`. Private reply receipt `public-smol-local-reply.private.json` SHA-256 `29a969e9`; persistence receipt `public-smol-persistence.private.json` SHA-256 `016bd221`; reopen-route receipt `public-smol-reopen-route.private.json` SHA-256 `785b9bc9`. | After exact approval, a fresh preflight showed 11.678% free and distinct public/holder identities. Only the reviewed engine tree was installed. Marketplace showed SmolLM2 available and opened the exact local offer in signed-in Assistant. One short prompt made one bound run; its execution-binding hash recomputed from the run offer, installed engine and verified GGUF. The journal has one completed terminal event and output SHA-256 `5cfdbfc7`; the UI showed the same reply. A second gateway restart preserved that conversation and model selection. Marketplace still showed Available on this device. The installed GGUF inode, mtime and hash and preparation count (11) stayed unchanged across reopen; fresh Kubo Bitswap reported zero received blocks/bytes. A 32-sample monitor saw minimum free space 11.664%, minimum MemAvailable 12,446,532 KiB, peak engine RSS 275,492 KiB and maximum load1 2.52. Public and holder keys, catalogue, provider, components and sources remained unchanged. Independent installed review found no P1/P2. | The first Get had the model in the seed’s own IPFS store and lacks a transport route tag; it proves local admission, not delivery to a fresh Home. Three admitted records still show `activation_pending=true` after startup restored an available offer. The flag records the last attempt and does not gate that offer; startup should clear or relabel it. Two later same-admission aliases have unknown UI initiators and transfer bytes. Two additional completed runs at 15:02 UTC have distinct input and runtime bindings; their initiator is unverified and they are not part of the one-run acceptance receipt. The reopened installed UI path showed zero new package payload, but host-wide counters cannot assign byte-exact Carrier traffic. The reply exceeded the requested one sentence; Assistant has no 32-token control and the approved packet disclosed its 16,384-token request cap. This one low-duty run does not prove sustained inference or the other J3 criteria. A read-only 15:20 UTC check found 11.659% free, above the 10% floor; recheck before staging. |
| Isolated public-artifact Linux fixture | `.audit/public-seed-gate-61974/receipt.json` and `.audit/public-seed-gate-61974/engine-preflight-receipt.json` (the port is historical receipt metadata); copied public Runtime, provider and manifest hashes above, with separate identity and data root | The retained Linux data root and provider bytes match the copied public receipt; the current gateway uses the diagnostic Runtime in the row above. The pinned llama.cpp `b10516` engine receipt verifies 52 files and 10 symlinks, and the earlier running engine mapped matching libraries. The second existing typed SmolLM2 run has a terminal journal whose execution-binding hash recomputes from that engine SHA-256, admitted weights SHA-256 and bounded Linux profile. CPU, RAM and free space passed at that run. No new run was sent. Read-only DMI and cloud-init data identify a Hetzner virtual server. | The purchased server type/plan code is absent from inspected operator records and host metadata. Shared and dedicated CPU policies differ; plan-specific sizing is needed for sustained public inference, while one short low-duty run can proceed after engine, resource and authority checks. This isolated result is neither public Get nor proof of the local `a1573675` code path or signed-in UI. The next checks are in [TASKS Now](TASKS.md#now). |

The earlier public free-space readings are dated evidence. Fresh resource and
artifact preflight remains necessary before each later public action. The
bounded Get and local reply above close only this J3/MA1 slice. Sustained
service still needs a plan/capacity decision. Remaining work is in
[TASKS Now](TASKS.md#now).
Private receipts stay outside Git; completed detail remains in the dated audits.

## Mac controlled DNS answer change, 23 September 2026 UTC

The isolated Mac diagnostic broker now accepts literal `localhost` only in its
owner-scoped HTTP fixture. It checks every resolved address against
`127.0.0.1` and gives the accepted address to the HTTP client. An Inbox owner
approved the exact `http://localhost:50348` route. A controlled resolver then
returned `127.0.0.1` and changed its next answer to `::1` before the broker
dialed. The approved IPv4 sink received one request, and the IPv6 sink received
zero. A fresh request while the answer was `::1` returned 400 with zero new
sink requests; restoring `127.0.0.1` allowed one more approved request.
The final check counted TCP accepts before request parsing: two at the approved
IPv4 sink and zero at the IPv6 sink. A private resolver trace records the
answer flip and a successful live lookup that returned `::1`. The earlier
HTTP-only count and a trace without the resolver result remain preliminary
evidence in the receipt.

Source commit `dfc1a945` passed 14 broker tests, three URL parser tests and
bounded independent review. Its installed diagnostic Runtime matched the
built SHA-256 `49c841b0f0ea0c5e396b30d3e2a3babe1c43826298426fdaf4dd9d0fe451ef86`.
The controlled resolver ran only in that diagnostic process. The original
Runtime, receipts, fixture, components and authority manifest were restored
byte for byte, and the restarted diagnostic process has no resolver injection.
Both signed-in human Homes and diagnostic Home returned HTTP 200. No paid call
occurred. Receipt: `.audit/sec1-dns-rebinding/receipt.json`. This proves the
controlled localhost answer change; public DNS, public CA routing, Linux
production confinement, unknown-create reconciliation, external HTTPS
activation and real hosted acceptance remain open.

## Mac hosted destination boundary, 23 September 2026 UTC

The Runtime-owned broker now keeps the private diagnostic HTTPS certificate
authority in the exact owner decision. A CA change needs a new decision, and
the broker uses that CA alone for the approved loopback TLS route. The broker
still accepts only the exact private fixture URL at literal `127.0.0.1`, with
redirects and proxies disabled. Public hosted HTTPS remains paused.

Source commit `623affc3` passed 13 broker tests, three loopback URL parser
tests, the basic gate, and bounded independent review. The installed Mac
diagnostic Runtime had matching built and installed SHA-256
`e795fee17b15c369ec03a52a4888b720e15f348aef5a8ef771060b97f527ec6a`.
Using a dummy key, the approved HTTP sink received two requests. An unapproved
redirect sink received zero. Replacing the fixture hostname with `localhost`
returned 400 with zero requests there. This literal-IP fixture does not test
DNS answer rebinding for an approved hostname. An approved TLS request with a
certificate for IP `127.0.0.1`
returned 200; the same URL with a certificate for `wrong.test`, signed by the
same test CA, returned 400 and delivered zero HTTP requests. The diagnostic
binary, receipts, fixture and authority manifest were restored byte for byte;
the two signed-in human Homes and diagnostic Home each returned HTTP 200. No
paid call occurred. Receipt: `.audit/sec1-destination-boundary/receipt.json`.
This is isolated Mac proof. Public-hostname DNS answer rebinding, public CA
routing, Linux production confinement, upstream unknown-create reconciliation,
public activation and wider MA/AI/CR acceptance remain open.

## Owner Inbox hosted-route history, 23 September 2026 UTC

On macOS, Inbox now reads the Runtime's private hosted-route decisions for an
admin passkey owner. It shows Pending, Approved, Denied, Ended and Expired with
the exact origin, recipient, payer and expiry. The owner can End an active
route in Inbox. Finished records move into private write-once history files
when they expire; active decisions remain the sole dispatch authority. Shared
Home notifications do not carry these route facts. Public HTTPS remains paused.

The diagnostic Home at port 61971 ran source commit `297f699e` with built and
installed Runtime SHA-256 `4b6b6fc0d0e87a2824bf429fcf073ac3aa2f42f1f57b1061dd2219357a7c7f7e`.
Its installed and served Inbox file matched source SHA-256
`848c7637f5113a0a480916dc1e85515d8edc93d6ace95cda23b1f400c9b80a03`.
In the installed Inbox, keyboard Approve changed Pending to Approved, and
keyboard End changed Approved to Ended. A new System Validate with the same
dummy key returned 400 after End and made zero new controlled-sink connections;
before approval it also returned 400 with zero connections, while the approved
validation sent one exact request. Denied remained visible without an End
action. The diagnostic binary, Inbox file, installation receipt and private
authority manifest were then restored to their original hashes; all three
Homes return HTTP 200. No paid call occurred. The source tests, Inbox browser
smokes and bounded independent review passed. Receipts are retained under
`.audit/sec1-inbox-history/`. This is isolated Mac fixture proof, not public
HTTPS or Linux product acceptance.

## Mac hosted route approval checkpoint, 23 September 2026 UTC

Runtime now requires an exact owner Inbox decision before the macOS hosted
broker validates a key or dispatches a matching provider request. The private
decision records provider, method, destination, recipient, payer, purpose and
configuration hash. Approval lasts ten minutes; System can end it early, and
the broker rechecks the decision and its run-bound grant during dispatch. The
public HTTPS route remains paused.

The diagnostic Home at port 61971 used a dummy OpenRouter key and a controlled
loopback sink. Before approval, System Validate returned 400 and the sink saw
zero requests. An admin passkey Inbox approval allowed one validation request;
System End then made the next validation return 400 with no new sink request.
The built and temporarily installed Runtime matched SHA-256
`424c3aeb13f4132cf9a0b601b638dc96bc588cfb5c9e970be12fd99206a07ac1`.
The diagnostic Runtime, install receipt and private manifest were restored to
their original bytes after the check. Evidence is retained under
`.audit/sec1-egress-installed/`. The two signed-in human Homes remained open,
and no paid provider call occurred. Independent review found no high or medium
source issue in this Mac fixture scope. Linux product confinement and public
HTTPS activation remain open.

## Current model delivery checkpoint, 22 September 2026 UTC

System Models now supplies the model editor and Approval Lens selection;
Assistant keeps one model selector. Installed keyboard and responsive checks
passed. A real Jev sample evaluation completed, and reopening it reused the
saved journal without another provider request. Marketplace projects the
existing grant's offer facts and hands the exact offer to Services or Assistant.
A new Consumer-to-owner service grant request awaits the owner Inbox decision.

Local commit `6a7004c2` adds the real hosted Assistant approval flow. Consumer
Home retained a short pending Venice prompt across restart; its direct Review
in Inbox action opened a card naming the requester, recipient, payer and
continuing scope, with Jev defer advice. Both signed-in Homes serve matching
Home, Assistant and System scripts from the source checkout, and the six
focused Jev tests and responsive Assistant replay smoke pass. The person
approved the Consumer connection in Inbox; its saved Jev record has
`human_decision=approve` and no actual outcome. Explicit continuation, terminal
Venice output, ending access and a later refusal remain to verify. The older
Consumer grant for the owner's shared model has expired. Consumer sent one
fresh service request, which now waits in the owner Inbox for a person's
approval. The hosted Assistant request and service grant are separate actions.

Hosted HTTP is paused on the installed local candidate while Runtime network
authority is built. System Validate and Save return a pause error before key
validation. The model provider refuses external adapter dispatch and queued
hosted work before HTTP. Consumer System shows the saved Venice and Jev
connections as paused and disables hosted Use in Assistant. Owner Assistant
completed a new local SmolLM2 prompt after installation. The diagnostic Home
returned four pause errors for OpenRouter/Venice Validate and Save with zero
controlled-endpoint connections and unchanged provider config. Built and
installed Runtime and provider hashes match on the three Homes; System source,
installed, and served files match. The 18 recorded protected files kept their
hashes and inodes. Runtime
SHA-256 is `90edb7fa1474d23ded52abee44f87c31a4f45f24eb083a8bef658ac6ef84176c`;
model-provider SHA-256 is `6dc1b33868761b9b3d8e92659d0f9f8911bfa4a6cd60ead700d4ced37842b11b`.
Receipt: `.audit/codex-hosted-egress-containment-installed.json`.
This human-Home installation has an application-level pause; its native provider
is unsandboxed.
The saved Venice text request waits for Runtime-owned socket isolation, an
egress broker, and explicit owner HTTPS consent. Production replay proof for
already-active HTTP jobs and Inbox history and revocation are separate open
clauses. The owner service request waits in Inbox; approving it would grant
service access, not provider HTTPS. No new paid call was made.

Local commit `364d0254` adds a macOS Seatbelt launch for the verified native
model provider. A fixture started through that Runtime bridge saw `EPERM` when
the child and its descendant tried a direct external TCP connection; loopback
worked. The already-installed model-provider binary completed a fresh SmolLM2
run under the same policy. The new optimized Runtime binary was built with SHA-256
`a8dc182a932a1c1f9cfd3c9f33eaf487fa6ece8906428aa7644107fe53e3c02a`.
It has not replaced the Runtime in either signed-in Home. Those Homes keep the
previous hosted pause. This first policy permitted every localhost port so
llama.cpp could run; a local relay remained reachable. The next checkpoint
below narrows that rule. Hosted HTTPS remains paused. Local proof notes:
`.audit/hosted-egress-design-scratch/seatbelt-milestone.md`.

Commit `8bd9f3ae` replaces the broad localhost rule with Runtime-selected
per-offer TCP ports and denies IPv6 outbound. A fixture child and descendant
received `EPERM` for external TCP, an unrelated loopback listener, and IPv6
at the selected port; the selected IPv4 port worked. The installed diagnostic
Consumer Home restarted on Runtime SHA-256
`9a4e324a1340c7918cfb15696fb86a70970de66f5898a41b10f85e9372cd6569`
and model-provider SHA-256
`26032f273ad2ba13c62c58bd1952db108debff8822dc19f16feb4d5488cbb19e`.
Its Home returned 200 and its provider child started. That installed provider
binary completed a separate fresh SmolLM2 run under the narrow policy; after
a forced provider kill, its guard and llama engine exited. The diagnostic
Consumer has no admitted SmolLM2 offer, so its Runtime-to-Smol path remains
untested. The two signed-in human Homes still run the earlier paused binaries,
and all 18 recorded protected files kept their hashes and inodes.

Independent review found that a selected TCP port becomes unowned before the
first local run and remains allowed after its offer is removed until Runtime
restarts. A second small Seatbelt test allowed one named Unix socket while
refusing an unrelated socket and external TCP for a child and descendant.
The installed llama-server accepts a `.sock` host and pinned reqwest supports
Unix sockets. That proof selected the private transport for the next checkpoint;
full provider egress authority, owner HTTPS consent, Linux confinement, and active
HTTP-job replay remain open. Receipt:
`.audit/hosted-egress-design-scratch/narrow-seatbelt-installed.json`.

Commit `f05bd171` replaces the selected TCP port with a Runtime-owned private
Unix socket broker for each initial local offer. The confined provider and its
engine have outbound permission only to that socket. The broker checks the
provider process, engine ancestry, exact local model routes, request bounds
and lifetime. A source-linked Runtime bridge test completed a fresh SmolLM2 run with the
diagnostic Home's installed model-provider binary, then removed its socket.
The diagnostic gateway has no admitted local offer, so a Home-launched SmolLM2
run on this transport remains open. The gateway restarted; its built and
installed Runtime match SHA-256 `54d2654e4e38d778ed2f8115fcaca504cf4f47fb7d0d16e930b06e8fa9768546`
and model-provider SHA-256 `63f8157bdc21fb5023a2de131c49bb4e7ca2b3f1ee6456494a78d6962c41c144`;
its provider manifest check and Home HTTP 200 passed. `just verify` with four
Rust test threads passed 4,566 tests with 28 ignored. The default run hit two
timing failures in model-provider tests, which passed alone and in the serial capsule
suite; a one-thread full run stalled in a custody child barrier. Both failed
logs are retained. Independent review found no new high or medium broker issue
and confirmed the proof scope. All 18 checked protected files kept their hashes
and inodes; both signed-in human Homes still serve HTTP 200. Hosted HTTPS
remains paused pending Runtime-owned egress grants and owner consent. Receipt:
`.audit/hosted-egress-design-scratch/unix-broker-diagnostic-installed.json`.

Local commit `5e16adaa` adds a Runtime-owned macOS hosted-effect socket. The
confined model provider sends hosted text, Decisions and HTTP-job requests to
Runtime; its Init configuration omits hosted credentials. System key validation
uses the same private grant and destination check. Public HTTPS remains paused.
On diagnostic Home 61971, System Validate and Save with a dummy key returned
400 before an active grant and made zero sink connections. One exact loopback
validation grant made one request; after revocation, the next request returned
400 with zero new sink connections. The diagnostic provider config and fixture
stayed unchanged. Built and installed Runtime/provider SHA-256 values match
`adfeaab2`/`c23d3e7e`, the provider manifest passes, and Home returns 200.
The two signed-in human Homes still return 200; all 18 protected hash/inode
records match, and disk free space is above 20%. No paid call was made in this
milestone. Independent review found no high or medium issue for the diagnostic
fixture. Public activation still needs verified owner consent, Runtime-bound
run/request and HTTP-job IDs, prompt active validation revocation, and target
process-isolation proof. Receipt:
`.audit/hosted-egress-design-scratch/hosted-broker-diagnostic-installed.json`.

The next diagnostic build binds private egress to an active admin passkey proof
and exact Runtime-issued hosted run/request. System Validate/Save uses its
verified Home launch proof, and the broker accepts a model run only after the
Runtime provider pipe flushes its typed create request. Local Smol runs do not
enter this hosted table. Source tests covered the denied and exact fixture
paths. Diagnostic Home 61971 has built/installed Runtime SHA-256 `fe74ea8f`
and provider SHA-256 `c23d3e7e`; its dummy-key System route made zero sink
connections for absent or wrong admin proof, one for the exact grant, and zero
new connections after revoke. The provider manifest passes. The two human
Homes remain signed in and return 200; all 18 protected hash/inode records
match. Public HTTPS remains paused. Installed model-effect run binding and
HTTP-job identity, active validation revoke, per-platform process isolation,
and public owner consent still need proof. An unobserved hosted completion can
hold one of 4,096 run slots until its two-hour expiry. Receipt:
`.audit/hosted-egress-design-scratch/admin-run-installed-receipt.json`.

The installed diagnostic Homes passed ten groups of hosted lifecycle checks.
Fresh destination wrong-principal, unapproved, revoked and expired requests each
returned a bound pre-dispatch refusal with zero provider calls. Earlier runs
remained readable and cancellable. Private and shared requests respected one
capacity limit. Exact key rotation preserved the running worker and changed the
next request's credential. Disconnect withdrew the shared offer, retained the
other connection and preserved completed-run access across restart. Cancellation
and owner restart reported settlement unknown where backend settlement could
not be confirmed; neither replayed a create. These are loopback fixture results,
accepted by independent review. The prior live provider and ordinary Home
receipts retain their separate scope. A separate 763-byte Content fixture stayed
in preparation with its reservation charged while the shared provider was frozen
for 20 seconds after Cancel. Releasing the provider completed cancellation;
restart caused no new read. The preallocated admission identifier did not imply
admission: zero bytes completed and no admission directory existed. Terminal
cancellation of a frozen provider remains open. A separate running-provider
check held an HTTP response body: the socket drained after 5.002 seconds, then
cancellation settled with zero reserved bytes. A separate four-byte read
succeeded, and restart preserved the terminal journal without replay.
On 23 September, a source-only request-exclusive bridge test sent a bounded
`cat` request (`max_bytes: 763`) to a dedicated mock provider, froze it, then
reaped it after caller cancellation in 0.43 seconds. The bridge pipe lock was
released and shutdown was idempotent; independent review found no blocking
defect. The mock did not transfer 763 Content bytes or exercise product
settlement. The installed shared-provider result above remains the acceptance
result, with frozen-provider terminal cancellation open.
A later source-only two-bridge test kept an unrelated provider responsive
while the dedicated child was frozen. The unrelated bridge returned exactly
763 synthetic bytes before and after cancellation; the frozen child was
reaped. Test failure cleanup also reaps the child. This
supports process isolation only. The installed Content path still uses one
shared registry and `ipfs` target for fetch and failure drain; it has no
request-owned route or cancellation handle. No installed terminal-cancellation
claim follows from the two-bridge test.

Use now records Keep with a new reservation. Explicit local selection waits for
a verified catalog mapping and Keep acknowledgement. Remove binds the caller,
checks foreign claims before withdrawal and preserves Keep until removal
succeeds. Independent review accepted the removal and recovery contract;
87 preparation tests and 43 Assistant tests pass. The combined gate passed
4,559 Rust tests, with 27 explicitly ignored checks, plus formatting, lint and
source/UI checks. Both human Homes run Runtime
`86337f318edb33460eb8b5eacbb3db6f6627c3e1956bb2497818d955580fd193`
and model-provider
`0be8144ca5785fc69facbfd64bf97a48cc062cfa5965b564bf8928d8d29b1cd3`.
Nineteen changed browser files per Home match source, installation and served
bytes. All ten protected configuration/key files and six SmolLM2 artifact
hashes, inodes and modification times remain unchanged. The isolated installed
fixture proves new Use creates the caller's Keep before cancellation. Human
Open and explicit picker selection retain SmolLM2; reload restores that choice
and the completed conversation. Marketplace passes visual checks at 390, 768
and 1280 pixels; keyboard navigation reaches the horizontally scrolled mobile
categories. These checks preserve the separate frozen-provider and fresh-grant
acceptance gaps.

The following paragraphs retain the earlier five-outcome regression evidence.

The unpublished closeout source preserves the security parent and original
Carrier/preparation work. A fresh source-built Mac Home completed the signed
SmolLM2 Marketplace-to-Assistant journey through direct Carrier from a distinct
seed Home. The 144,835,448-byte closure and all five file hashes match the signed
catalog. Runtime activation completed; two separate Assistant runs returned
terminal output from the admitted model.

Conversation, model selection and an unsent draft survived reload and an owned
Runtime restart. Warm reopen with the consumer's seed route withdrawn used the
same weights hash, inode and modification time, with zero Carrier UDP traffic.
Admission retained one 443,091,560-byte quota charge and the owner's Keep claim.
The source-built Runtime for this accepted journey is SHA-256
`b24f3c7cf68b9ecd7e52034e081d45c7482cd539f85cd8927c826a59d2772afd`.
Independent review accepted the artifact and UI receipts. The human passkey
session stayed signed in. Marketplace polling stopped before the longer transfer
finished; Refresh restored progress while the backend operation continued.

Runtime `b20622552145c906bc409cea9aa59b417b62970886e71a181095be63f055ae24`
completed a real private Venice Assistant request through ordinary Home on
`qwen-3-8-flash`. The receipt binds the visible turn, canonical
input hash, named offer, request, run and terminal response. Provider usage and
billed cost remain unknown. The named model configuration and stored key hashes
survived the subsequent install and restart of Runtime
`fe44fe8aae0212fc31414ce03b33c9dc8fc341414703b1ccedbba141d0e5d188`.
That Runtime also completed a fresh lighthouse prompt through ordinary Assistant,
with a new request/run and reported `qwen-3-8-flash` terminal output. Built and
installed Runtime, model-provider and changed capsule files match; installed
provider verification passed on both test Homes. A new ordinary Assistant run
also completed on the admitted local SmolLM2 model, with an independently checked
input hash, request, run and terminal reply.

A later installed Runtime (`45490bf7a0f61155edf3e3a1325585c2ebf90ae4a1918fde53b1b960d67576d4`)
completed real Jev advice through ordinary Home. The provider reported the exact
catalog identity `typesafe/jev-1.13-20260917`, 591 input tokens, 85 output tokens,
and US$0.000024822. The contract result names the selected `typesafe/jev-1.13`
and retains the provider report separately. Inbox shows defer, medium risk and
56% confidence. The human approved in Inbox. The prompt was then resubmitted through
Assistant. A distinct DS4 run completed with the matching input hash and terminal
reply; OpenRouter reported `deepseek/deepseek-v4.1-flash` and US$0.000189. The
connection-scoped decision, create acceptance and terminal result remain separate
facts in the receipt. An earlier response-identity failure stays in the evidence.

Home Edit retains an existing same-provider key when its key field is blank.
Repeated installed saves preserved all nine connection identities, stored-key
hashes and list order. Warm local engine retention permits idle hosted additions
while active and unresolved ownership guards remain in force. The two signed-in
Homes discovered each other and accepted a contact through ordinary Home; the
owner shared one named Venice connection and approved the consumer request in
Inbox. The consumer listed its authorized remote offer before a paid request,
then received “The mountain stream rushed over cool stones.” Its grant, named
instance, request, canonical input hash and run match the owner records and
provider journal. The provider reported `qwen-3-8-flash`; usage and cost remain
unknown. One journaled dispatch does not establish an independent billing count.
The conversation and selected remote offer survived consumer reload with the
completed provider journal unchanged. The consumer holds zero provider keys.

The model-only request mailbox and secret-reference sharing repairs are installed
on both Homes. All 29 remote-model tests passed, including fresh-request authority
denials, pause/disconnect withdrawal, regrant recovery and continued access to old
runs. A private/remote same-instance concurrency test passed. These are source
fixtures. An authorized installed pause and fresh consumer prompt produced no
new owner run or provider journal; the prior completed run stayed intact. The
exact share, configuration and existing grant were restored. A new chat and
explicit unpaid model refresh recovered the same remote offer. The consumer
conservatively retained unknown acceptance because the current error classes
can also arise after dispatch. At that baseline, precise refusal copy still needed a destination
pre-dispatch assertion and the installed negative matrix remained open. Both
owned Homes then ran Runtime
`acb5011232841a3cde788494b47172bf8d0503ab2665c7f1715e56ac3d54dc81`
with matching built and installed provider/capsule hashes. Normal restart,
served Inbox parity and installed provider verification passed. Assistant's
reviewed disclosure patch also matches installed and served bytes; it names the
provider Home as payer and both prompt recipients while leaving retention
unreported. The owner's nine keys and provider configuration remain unchanged.

The installed UI now keeps model preparation polling active until a terminal
result or view closure. A regression check passed 131 polls and verified timer
cleanup. The optional `carrier_bind_addr` setting gives a configured direct
bootstrap a stable listener; an invalid or occupied explicit address stops
startup. The basic AGENTS gate and full `just verify` passed on the combined
source, including separate capsule workspaces and the Browser local-exit helper.
The server suite passed 2,186 tests with 16 opt-in skips; the model provider
passed 215 unit tests and five process tests with two opt-in skips. Live IPFS
and documentation examples retain their opt-in skips. Focused provider, gateway
and UI checks cover the later Save, canonical identity, sharing and disclosure
repairs; the full-suite counts above describe the earlier combined candidate. The five bounded handoff outcomes are verified with these
explicit source and installed evidence scopes. The local candidate remains
unpublished; Marketplace service entry/access projection retains its own work.
Public installer, reverse Mac dial, approved relay, indefinite
provider-hang cancellation and full Qwen delivery retain their separate gates.
The following dated inventory preserves earlier evidence and public state; it
does not describe this new local candidate as published.

## Earlier model delivery checkpoint, 19 September 2026 UTC

The model-delivery source line is `feat/remote-services`
`8da670b5c3f8a4409a7ac5cb0d14f413ee3b105e` on `origin`. Working closeout
branch `feat/0.7.1-model-assistant-closeout` is
`7c65a7cef309eded5def73ed528cbfcdb9b608e4` tree
`66df724f8fc1bcdfacf502c1f92e71e3785bca50`. It starts from published
`fix/0.7.1-security` `54355973e737f018e7d898a74449f9b04aaef26c` tree
`b6442321b77f02382b821f27dc11f397559b2be6` and must merge with or after
that parent. `d1625aaf` amends unpublished `ddfa50f3` so prepare seals
an owned Kubo repo root to exactly `0700`. `6144fb2a` reports hosted
selection facts in Assistant. `ee8b8cd8` records a Jev Approval Lens
skeleton on hosted Assistant `runs_create`. It writes
`recommendation=unavailable`, `risk=unknown`, `confidence=0`, and
`needs_human_review=true`. It does not call Jev or show Inbox. `d8cd6a05` records MA1-MA4
installed evidence. `7c65a7ce` restores Assistant user prompt text from
`modelText`. The sealed Linux ipfs-provider SHA-256 is still
`8af761a111fdb8b962437e4d530da060569e19defe64c420a26f4e8005227731`. Darwin
candidate Runtime SHA-256 is
`a908e7b7c1d67b5107eba21f12847d4efecc1ee96601211af37d2ec80ab9f32c`. Isolated
Linux `61942` Runtime SHA-256 is
`07a8477cb7582c9d9e3099000716cc5e2fa3115353d94f6c7c6911960639f5a9`. The
security parent still holds six committed security slices plus the later
object-provider lock and two-platform admission commits:

- `4b1a8a47` keep private data roots off `/capsule-data`
- `9ffd25c7` require tokens for shell content writes
- `cbf6a58a` persist capability revoke across serve restart
- `71958e9b` write `signing_key` without following links
- `51bdcd1f` keep guest session bearers out of Home JSON
- `bd71cc2e` install receipt-bound binaries and owner-only media tools
- `42e31e30` keep object-provider lock aligned with signing-key libc
- `a48ed2a7` admit two-platform stable publication

Launch principal and Home grant binding stay on this tree. Carrier line,
frame, archive, and HKDF stay later. Isolated one-Send qualification
`isolated-qualification-61770.json` SHA-256
`ad7aa591c39ba1c94207fa580233f379061a7a576a4541ed4d5a40d67b9bb687` stays
protected. Combined Mac journey `isolated-qualification-61780.json` and
Darwin candidate `isolated-qualification-61800.json` stay protected.

Public 0.7.1 is live. `https://elastos.elacitylabs.com/release-head.json`
reports version 0.7.1 and latest release CID
`QmT16KDvZqA4wQc74ssFgy8JJN578Z64AAAhzYvkoE4NF8`. Live signer DID is
`did:key:z6MkrFPDgDi98Ek6AFHM3VT9bVJytnDf5mfHAV6gyrD5frYj`. Installed public
Linux Runtime SHA-256 is
`63b3cdb06a45523f008fc4e67b50eb795a074890b0a3e839cba2ffa5d532d830`. Darwin
installer bytes match
`82593dfc71f53cffa2dd5fd8c68cbaf4a048890929bf08a85aeb83354558c29f`. Catalogue
SHA-256 is
`c81d43574da91ad278300eda59330cfdf1fd6a924ef2ebf63658ab0e261d2f2c`.
components.json SHA-256 is
`40e7835833e73378d370283b28eefe1e4432773d4b256f0ede2405b8c0a8f4f0`. The old
0.1.2 head `QmVLFNQfW6V2LuXCX5xAq1jUmQrReE294Fb2NvETWgNbRk` is rollback only.

Public Home https://elastos.elacitylabs.com/home/ serves 0.7.1 and preserves
the existing DID, passkeys and `sources.json` hash
`96b62da57dbfe5d2245403464e99ec65b80968ad96b612724acdb1882aab0676`. Marketplace
lists verified SmolLM2 CID
`bafybeidy5kfvqwg6g6pfgdfwslmhijosbeskt5b2duqdqxnc7e6fwmr72y`.
Those earlier public Get
attempts failed at MetadataRead on the then-published ipfs-provider. The
isolated capacity probe found the Kubo `0775` repository blocker; the public
journal's `metadata_read` label alone does not identify its internal error.
Isolated Linux Get with sealed ipfs-provider SHA-256
`8af761a111fdb8b962437e4d530da060569e19defe64c420a26f4e8005227731`
admitted the package, streamed Assistant `Pong`, settled Stop as
`settlement_unknown`, and reused the same weights after Runtime restart
with Bitswap payload received 0. Catalog status for the admitted
operation is `admitted` and `dispatch_ready`. Workspace PUT on that
Linux Home needs principal-root protection. First-owner passkey
enrollment on that Home is blocked by `require_unowned`. A separate
isolated Linux Home enrolled an admin passkey through virtual-auth.
Isolated Linux `61942` completed Marketplace Get of SmolLM2, Open in
Assistant after a mode-`0500` llama.cpp engine bundle, a Ping reply,
reload persist, full Runtime restart persist, and Stop
`settlement_unknown` on run
`run:sha256:b7aace10d6d00498755177be293dc57357812aa70d413d5e7592649085e338c3`.
That Home now runs candidate musl Runtime SHA-256
`07a8477cb7582c9d9e3099000716cc5e2fa3115353d94f6c7c6911960639f5a9`.
Darwin candidate Runtime SHA-256
`a908e7b7c1d67b5107eba21f12847d4efecc1ee96601211af37d2ec80ab9f32c`
from `7c65a7ce` stays off holder `61680`. Assistant Settings on `61942`
reports requested, resolved, provider, limits, privacy, cost and
fallback. A renewed remote Qwen grant
`services-remote-model-grant-503a0102ae5df303` produced a Carrier Ping
and Home reload persist with zero extra `runs_create`. A later full
gateway restart restored the same remote model, the draft
`keep this remote draft`, grant `503a0102` as reachable, and
`createsAfterReload` 0. Hosted Chat
Completions credential is the remaining operator input for a live
hosted route. Candidate remote Stop and grant revoke stay pending.
The ignored Kubo prepare fixture passed. A bounded no-holder Get on a
temporary signed tiny catalogue failed at MetadataRead with zero
payload. Isolated Mac holder and isolated Linux consumer share
collaboration-network startup config
`elastos.collaboration-network.startup-config/v1` with no model offer
in that file. People connected. Linux Ask to use and Mac owner
approval produced grant
`services-remote-model-grant-a21916fd3fd3241b`. The first remote Qwen
reply is run
`run:sha256:7bf00494cee50905ef135ccf3708893222c1c71885b0cd693691c23de9e4bbd7`
request `7b9429c1-c02b-4991-9a16-0acaabff6157`. Home reload restored
that conversation with zero `runs_create`. Remote Stop settled
`settlement_unknown` as run
`run:sha256:0d5a802737aba8e43f8ad028cddafa9c9470993451ab2dbabfdaabc766409592`.
Mac `61680` holds complete Qwen weights
`d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`. Seed
catalog SHA-256 matches Mac `61700`
`c81d43574da91ad278300eda59330cfdf1fd6a924ef2ebf63658ab0e261d2f2c`.
Stopping llama-server on `61680` left Home HTTP 200. Hosted Chat
Completions credential is the remaining operator input for a live
hosted route. Public Home stayed unchanged. A public ipfs-provider replace needs a
separate approval. After Kubo idle-stop, a
live repo can still pin that CID and later serve `_elastos_object.json`
(726 bytes) locally. The product bounded read uses
`offline=true&timeout=100ms` during backend restart.

Fresh Apple silicon install from `https://elastos.elacitylabs.com/install.sh`
completed ordinary Get, two Assistant replies, reload and Runtime restart with
zero payload transfer. Weights SHA-256
`c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a` at
144811072 bytes. Remote Qwen on public Home is blocked. The preserved Mac Home
still holds admitted Qwen locally and has no published Services share for public
Home.

Full R14, J1–J5, SA1–SA6, CA1, responsive UI, Browser, and remaining security
stay open. Browser stays paused.
The parent `feat/remote-services` line matches the Cloud-reviewed PR65
integration. The freeze ancestor is
`20ac3f628aea683e2cefff7cf7c056852af3365c` tree
`99beff2bf89510d0744c8303ea5757bc075d8e8e`. Historical D1 freeze
`7690aabc6acb4f125b84b7ec0117e3e406898b91` tree
`e7d8d0e1ac2b343e3104f7eefd1dcfeb3fba6bbc` keeps its own installed bindings.
Installed results retain their own binary and patch bindings. Public Home
https://elastos.elacitylabs.com/home/ is the 0.7.1 preview target. Preserve
existing Homes.

The signed catalogue contains Qwen and SmolLM2-135M-Instruct Q8_0. Mac and seed
holders retain the small package. Complete Mac-holder delivery was hashed, and
an isolated Mac consumer admitted the package through Marketplace authority.
The installed weights match the published 144,811,072-byte SHA-256. Coordinator
verified ordinary Assistant Send on the isolated consumer Home. Reload and
restart preserved that history. Earlier accepted Qwen receipts keep their
original scope.

The reviewed Linux CPU and multi-model startup delta `e84dd728` is integrated.
The two-order startup, Mac symlink and catalogue identity tests passed. The
managed-Home catalogue propagation repair and its three regressions are present
in source; a fresh installed Home without the diagnostic manual copy remains
unverified. Preparation cancellation and a total deadline across holder attempts
remain open. Honest inference `settlement_unknown` remains a distinct contract
from responsive download cancellation.

Eight seed-only 64 KiB reads took 18.516 seconds, including 18.513 seconds inside
the Content requests. Seed-local Kubo read the same amount in 0.04 seconds.
This rules out the driver as the main delay; it does not identify the slow
Runtime, Carrier or network stage. On the installed Isolated WAN lane, dest
copy of 8 MiB took 2.059 s and dest copy of 64 MiB took 4.524 s, both matching
SHA-256 `e6d36653…` and `f8550531…` with UDP 55180 and zero relay addrs. A
linear Qwen forecast from the 64 MiB sample is 415.9 s with 3184 s headroom
inside 3600 s. Source dest-path cancel, silent-peer idle, truncated data and
retry passed. Isolated Darwin 61953 dest-receive `elastos` SHA-256
`79f51380…` and Isolated Linux 61954 holder `elastos` SHA-256 `cc77e402…`
run the header-then-file dest-stream candidate. Isolated 61954 holds a
recursive pin of Qwen CID
`bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi` after holder
restart. WAN dest copy of `weights.gguf` wrote 6,169,341,984 bytes in
1092.804 s with SHA-256
`d784ce9eda1a5a7b51e8f705a9e6310844bf4f173654d115823c775fdea56d43`. Isolated
Darwin 61953 reconciled those verified bytes into inventory admission
`2d690d11…` (6,169,366,387 bytes) on installed `elastos` SHA-256
`d3aec5f097fb1c8b047f587fb12624e998c8e7b01eb191d2cba03a497b32553c`,
source `9a922faa`, built `8d9b13d911ec638f…`. Marketplace `content.status`
reports `dispatch_ready` true. The 16:32 Assistant UI Ping receipt is
partial first-delta text (53 characters, 1508 ms). Journal run
`run:sha256:7f792ef3…` on offer `model:00c7b9dd…` completed with 343
characters of terminal output. Receipt
`.audit/ma3-2-qwen-assistant-ping-journal-settlement-receipt.json`. The
09:06 progress receipt stays transferred-bytes history. SmolLM2 admission
`657460e0…` stays. Cold Isolated 61957 ordinary Marketplace Get `b280a1b9…`
admitted 6,169,366,387 bytes from Isolated 61954, weights SHA-256
`d784ce9e…` match, `dispatch_ready` true, offer `model:00c7b9dd…`. Assistant
Ping on that copy completed with 520-character terminal output, journal
`run:sha256:e7ad986b…`, request `3c3e05aa-5fca-4ddd-b764-bb2384957104`.
The original 16:32 UI receipt stays partial first-delta. MA1 is Active as
separate sub-gates. Accepted 12. Waiting 2. Needs one named proof 1.
Receipt `.audit/ma1-acceptance-matrix-receipt.json`. Current signed
catalogue Refresh is accepted. The successor catalogue waits on a catalog
signing key. Cancel, retry admit, the live-window failed read, expiry
recovery of a complete hashed stage, and interrupt restart recovery are
accepted. Expiry recovery does not extend `created_at+3600`. Both
acquisition orders still need one proof, and that proof needs a Qwen
transfer this card does not start. Smol selection and the completed run
`run:sha256:49bd09ab…` are accepted. That receipt has an empty request id.
Stop is accepted: 61953 is honest `settlement_unknown`, and 61942 is
confirmed completed. Coexistence removal, byte reuse, and Chosen model
unavailable are accepted. Active-run Remove consent and local retention
consent are accepted on disposable 61956. Global sole copy stays Waiting.
MA2 is Partial/Waiting on DirectOnly Ask 61942 to 61680. Mac services
runtime node `8d03e7b1` connected outbound to DirectOnly 61942 node
`ede679a3` in 0.236 s. The dialed ticket had two public addresses and zero
relay addresses. `list_peers` then showed that one peer. The Mac ticket relay
was left unused. This proof sends no Ask. Live prepare is
`created_at+3600`. Recovery does not extend or close that budget.
The completed MA4 fixture Home 61958 was removed after census. The 61956
Ping after Cancel/Retry admit timed out and stays outside the accepted
Smol execution clause. MA2 Ask delivery stays open after the Mac outbound
route to 61942.
Release test `model_preparation_restart_reconciles_expired_rename_after_exact_hash`
passed. Restart status admits an expired complete unrenamed stage after
authority, catalogue, capacity, index, and CID/hash checks. Receipt
`.audit/ma1-61956-expiry-parent-receipt.json`. Mac Content serves the pinned Qwen closure from the existing 61953
Content repo. The managed Home runtime attaches to that repo. One Kubo holds
the repo lock. Its parent is the Content ipfs-provider. The signed CID stayed
recursively pinned after the Kubo restart. Authenticated Content read 4096
bytes and then 65536 bytes of `weights.gguf`. Both hashes matched the admitted
file. Carrier availability read the same ranges. The holder ticket has two
private addresses and zero relay addresses. The seed has no on-link route to
those addresses. A separate DirectOnly seed consumer, node `fc3c1262`, has two
public addresses and zero relay addresses. The Mac Content runtime connected
outbound to that ticket. The seed then read 4096 bytes in 0.601 s and 65536
bytes in 1.038 s through ordinary Content and Carrier. Both hashes matched the
admitted file. Availability policy was `carrier_provider_invoke`. During the
4096-byte read, 10 UDP datagrams used the consumer's published ports, and the
longest was 1452 bytes. The seed consumer repository stayed 136 KiB. Free space
stayed above the 10 percent floor. The managed child repo stayed 285,892 KiB.
Holder 61700 still has no Kubo child. A fresh DirectOnly seed Home at
`127.0.0.1:61962` sent one Marketplace `content.use` for CID
`bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi`. The
three-copy charge stayed above the 10 percent floor. Mac holder node
`3ddc07c6` connected outbound to managed Home node `b043677c` in 0.199 s.
The preparation read used the gateway carrier. That carrier joined direct
gossip with 0 bootstrap peers. The local offline read of
`_elastos_object.json` returned HTTP 500. Operation `bf2ff7d0…` stopped at
`metadata_read`, provider error kind Provider, 0 completed bytes, and
`cancel_requested` false. `content.cancel` stayed declared. The attempt sent
18 public UDP datagrams and 0 datagrams to seed holder 61954. The gateway
repository stayed 128 KiB. Assistant execution remains unproven for this
Linux consumer. The owned transfer is operation
`2feb233d8dea49d7e67873a8a850335693c03b47e6438b1e772747ab2d293c9a` on
disposable gateway `127.0.0.1:61962`, managed Home `127.0.0.1:44117`,
marketplace `content.use`, same Qwen CID. It is not operation
`bf2ff7d0…`. `content.cancel` returned HTTP 200 and set `cancel_requested`
true. The journal stayed `preparing` with `reserved_bytes` 18516684377 and
`completed_bytes` 24403. The worker stayed in `cat_to_path`. The last
gateway warning was 2026-09-21T22:38:27Z at elapsed 900 s. One SIGTERM to
the disposable gateway stopped that consumer within 15 s. Seed holder
61954, the Mac holder, its Kubo, and the human Homes stayed. The journal
is now `uncertain`, `failure_phase` `weights_read`, `cancel_requested`
true, and `reserved_bytes` 18516684377. The reservation is still held.
This stop is operational containment. The running worker did not settle
inside that process. One normal restart of the same installed gateway,
pid 955407, binary SHA-256 prefix `cc77e40279d5e595`, then one
`content.status` for this operation wrote `cancelled`, `reserved_bytes`
0, and `completed_bytes` 24403. No admission directory exists. The worker
lock was free. Recovery removed the stage weights file. The ipfs-repo
grew 1,759,775 bytes and logged no `cat_to_path`. The Mac Qwen pin stayed.
Seed holder 61954 stayed. Managed Home `127.0.0.1:44117` did not listen.
SmolLM2 stays unstarted. Mac free space was
67,666,980 KiB of 482,746,452 KiB. Seed free space was 44,790,288 KiB of
314,748,412 KiB. Installed elastos SHA-256
`cc77e40279d5e595b8690f982c86517a0f0b8297e9c78390d6d0542fe853a4c1`
matches preparation.rs at `20ac3f628`, where the stop message is line 1075.
That revision and current source read model bytes with `fetch_model_part`
in 64 KiB bounds. The installed binary contains
`ipfs-provider cat_to_path left no dest file` and `carrier-content-fetch`.
Both strings are absent from `20ac3f628` and from HEAD. The live
`cat_to_path` read was that overlay. Current preparation ends a held
bounded read when `cancel_requested` is set. The existing drain and
`settle_failure` path then releases the reservation.
`model_preparation_actual_fetch_cancel_waits_for_held_read` used a 16-byte
GGUF fixture, cancelled while the weights read stayed held, and reached
`cancelled` with `reserved_bytes` 0, no stage, and no admission within 5 s.
Two status calls and one fresh owner status left that record in place and
started no worker. The same run passed the silent-holder cancel test and
the restart drain test. Disk before that run was 67,613,820 KiB free of
482,746,452 KiB. The source test passed. The disposable seed lane then ran a debug elastos
from HEAD `e387c9ad` plus the uncommitted fetch helper. Built, installed,
and the running process share SHA-256
`6cc2ab44ecc46b28571c669510b8e39394d223fa0219ba18d52305de1d0187c3`.
The file size is 454001624 bytes. Proof gateway `127.0.0.1:61963` used a
763-byte fixture. The harness returns the ipfs-provider child when Kubo is still absent.
One `content.use` then observed `op=cat` on that bridge. `kubo_pid` was
null at that identification. The proof stopped the provider after
`op=cat` was sent. `content.cancel` ran while the read stayed open.
During the hold the journal stayed `preparing` with `reserved_bytes`
8587505. Time from the cancel request to `cancelled` and `reserved_bytes`
0 was 1.29 s. That interval includes the deliberate SIGSTOP hold of
1.389 s. After SIGCONT the terminal state arrived in 0.201 s. The cat
settled before `runtime_prepare_backend`. This result is a controlled
drain-order proof. The provider resumed after the bounded hold. The
stage directory is absent. The admission directory is absent. Two
status calls and one restart status matched. Those calls sent no new
`op=cat`. The loaded linux ipfs-provider matched its manifest checksum.
The gateway log shows carrier online with 1 relay. Canonical `carrier.rs`
at that binary starts `CarrierNodeNetwork::Public`. `ELASTOS_CARRIER_NETWORK`
is absent from that binary. Parent accepts this local cancellation result.
The acceptance covers the controlled drain, the zero reservation, and the
idle restart. An indefinite provider hang stays a separate gate. WAN Carrier
stays a separate gate. Seed holder 191661 stayed alive. The Mac protected
processes stayed alive.
The 22 September Codex review traced 306 seed build inputs to this source.
The recovered dirty files and manifests matched the recorded installed build.
Review then found two additional constructor gaps: operator calls selected N0,
and the direct listener ignored explicit binds. Both now use the shared Carrier
policy. Eleven narrow tests passed, followed by six affected tests after keeping
isolated fixtures on ephemeral ports. The repaired Mac Runtime built successfully;
its fresh source-home installation is in progress. This new installed journey
remains pending. The retained admission charge is defined quota:
`3 * 144835448 + 8388608 + 196608 = 443091560`; reuse aliases charge zero.
Private evidence: `.audit/codex-goal1-lineage.json`.

The reviewed CR1 selector is back in canonical `carrier.rs`. Unset and
`direct` start Isolated. `public` is an error. `ELASTOS_RELAY_URL` on that
path is an error until CR3 names an approved ElastOS relay. Isolated strips
foreign relay hints before MemoryLookup and connect, and the connect hook
rejects a relay address. Eight policy tests passed on the seed. The
installed proof Home `127.0.0.1:61964` runs SHA-256
`16fb94adb6aba4d9e03445c35a8835c9e399416f9221e46a4ba89f6591a64556`.
Built, installed, and the running process match. The log says isolated.
The ticket has 6 IP addresses and 0 relay addresses. The loaded linux
ipfs-provider matches its manifest. The configured Mac holder ticket
already had 0 relays and 2 IP addresses. That connect timed out at
`metadata_read`. Operation `059969c3ba8f` stays failed with 0 bytes.
The seed holder already pins SmolLM2. One later `content.use`, operation
`8f885ed89d08`, read the 726-byte signed index and admitted 144,835,448
bytes. Weights are 144,811,072 bytes and match SHA-256
`c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a`.
The admitted journal still records `reserved_bytes` 443091560. Holder
191661 and lane 1004338 stayed alive. Proof gateway pid 1040783 stays up.
Qwen stayed deferred. Mac free space is 121731864 KiB of 482746452 KiB.
Seed free space is 40507532 KiB of 314748412 KiB. Full Qwen distribution
acceptance stays open.
Receipts `.audit/ma3-2-mac-holder-range-proof-receipt.json`,
`.audit/ma3-2-mac-holder-64kib-range-receipt.json`,
`.audit/ma3-2-seed-to-mac-route-receipt.json`,
`.audit/ma3-2-cold-consumer-receipt.json`, and
`.audit/ma3-2-qwen-cancel-closeout-receipt.json`, and
`.audit/ma3-2-carrier-policy-receipt.json`. The local split through
the MA1 record is `f6242b51`. The branch has no upstream. Live MA4
hosted credential is absent on 61680 and public live. The one-form System
Models OpenRouter/Venice Settings surface is a shortcut, not the MA4 product
boundary. Isolated Darwin 61960 proved two OpenRouter instances plus Venice on
that instance surface. Secrets stay in Runtime files mode 0600. Share of Jev
left DeepSeek and Venice private. Assistant selected Jev by name. Disconnect of
DeepSeek left Jev and Venice. Restart recovered those two offers. Receipts
`.audit/ma4-instance-pointer-ui-receipt.json` and
`.audit/ma4-instance-installed-receipt.json`. Historical Isolated Darwin
61958 one-form Settings fixture evidence stays on file and does not close this
corrected boundary. Isolated Darwin 61958 overlay-matched that candidate and
moved the live OpenRouter key into Runtime secret storage mode 0600. Offer id
`model:openrouter` stayed. Ordinary Assistant Ping on that instance completed as
journal `run:sha256:9c0b8c5b…` request `8cf4d28c-f123-4629-9c3a-fae0ba8afc56`.
The Assistant composer still showed `Ask on this machine`, a Think chip, and
`OpenRouter · cost unknown` on that Home. Isolated Darwin 61960 now shows
placeholder Message Assistant, one user-chosen name on the trigger, and selector
rows with a short route subtitle plus expanded facts at 1280, 768, and 390 CSS
pixels. Receipt `.audit/ma4-composer-truth-ui-receipt.json`. DirectOnly second-Home Ask stays
recorded once. Isolated Darwin 61960 installed this dirty candidate and
proved ordinary Home Inbox shadow through a local Jev-compatible provider
fixture. Pointer Approve kept the person as the authority. Venice, Alpha,
and Beta each keep one request id for recommendation, human_decision, and
actual_outcome. Records are mode 0600. Built unsigned elastos SHA-256
`4cae1446…` matches installed unsigned; codesigned SHA-256 `40b75856…`.
Receipts `.audit/ma4-61960-jev-overlay-receipt.json` and
`.audit/ma4-61960-jev-inbox-fixture-receipt.json`. This is installed
fixture evidence. Live TypeSafe/OpenRouter receipt and live-provider
acceptance remain open. Auto-approve stays off. PR #69 `7285cba` is
reconciled on this dirty tree without cherry-pick: fail-closed installer
rules, shared Browser protocol 2.1, and sign-in wording stay, while README
and Getting Started keep the deployed two-platform endpoint. PR #68
`4f7d863` + `a9d0b43` stays a later presentation slice. Receipt
`.audit/ma5-pr69-pr68-review-receipt.json`. Goal 1 classified and
repaired the four remaining combined-candidate source-gate failures:
stale Home-Agent vendor-ui target, stale home-agent size-0 prepare
fixture plus Darwin rustc/cargo mocks, stale Carrier
`elastos-identity` 0.6.0 versus workspace 0.7.0, and real clippy
`needless_return` in `setup.rs` plus MA4 test-double tails. Receipt
`.audit/ma5-goal1-source-gate-receipt.json`. The entropy pass kept demonstrated Home-Agent, offer-alias, secret-migration, and single-instance callers. It gitignores `/.audit/`. Receipt `.audit/ma5-entropy-receipt.json`. Full `just verify` waits. The dirty tree can be split into local commits. That split waits for an explicit request.
Live Venice and extra
named live Jev instances remain. Ordinary busy-Remove on Isolated
Darwin 61956 after a product Get is proved. Pointer Get admitted `2eb4c281…` at
144,835,448 bytes after a DirectOnly kubo pin from holder 61953. The first
pointer Remove returned HTTP 200 and saved `withdrawal_pending` while llama
stayed live. Repair of `content.reclaim` then returned HTTP 409 with copy Stop
the current reply in Assistant, then try Remove again, kept that admission and
weights SHA-256 `c4a3dd03…`, and left retirement none. After the reply ended,
one Remove reclaimed `2eb4c281…`. Reload showed Removed from this device.
Overlay codesigned `elastos` SHA-256 `9fa10cbb…`. `model-provider` SHA-256
`fa851ca1…` stayed. Qwen on 61953 stayed admitted. Receipts
`.audit/ma1-61956-ordinary-busy-remove-pointer-receipt.json` and
`.audit/ma1-61956-ordinary-busy-remove-parent-receipt.json`. Terminal and key-file
workarounds are rejected. Receipts
`.audit/ma4-installed-settings-fixture-receipt.json` and
`.audit/ma4-settings-pointer-parent-verify-receipt.json`.

The installer catalogue-before-replacement repair is integrated. Publisher
discovery reads health from one selected control coords file. Bootstrap comes
from ELASTOS_SOURCE_PUBLISHER_URL, or from that same control URL on a legacy
operator layout. Matching Linux native inputs are required for this candidate.
Complete three-platform W1 remains required for the release. A source review
push has its own source gates and exact-candidate approval; platform release
artifacts are not a prerequisite for draft review.

Whitespace, Home entropy, Browser entropy and required formatting checks are the
current source review gates. Installed and human acceptance retain their separate
scope. Browser is paused during model delivery. Preserve its prior receipts,
source and J4 obligations, plus all J1–J5, M01–M06 and shared-state criteria.
Current execution is in [TASKS.md](TASKS.md); target paths, process ownership and
raw proof identities remain in the private development-loop checkpoint.

## Published preview and review checkpoint, last verified 11 September 2026

`feat/0.7.1-integration` is the active working/review branch in
[draft PR64](https://github.com/Elacity/elastos-runtime/pull/64), based on
`upstream/0.7.1-dev` at `6c61c990`. Product checkpoint `06bf4e0f` preserves
both website heads and the reviewed contributor histories. The redundant website
refs and checkout are removed; the remaining donor/history gates still apply.

Public website and Home now run `259666222f12b21283131cc7926ba7d0c1a52e99`,
tree `419bdba6a09ed37e8176ddfda9c22c1a270f6054`. This extends the publication
closeout with reconciled dependency locks and `--locked` setup builds.
All 25 required Linux manifests passed locked offline checks, and the cached
Linux source-home build completed. Application source retains the reviewed behavior.

The authorized deployment at 17:42 UTC passed installed integrity, 573 artifact
file comparisons, running Runtime parity and public website/Home/Services/Assistant
hash checks. `/apps/home/` redirects to `/home/`. Account records, keys, user files
and provider settings match the preserved state. Migration and restart receipts
report zero migrated roots/objects. An owner-only identity directory and Runtime's
empty identity lock are the recorded metadata changes. Anders confirmed existing-account sign-in and saved work. The temporary stage
and bounded deployment rollback are removed.
The configured Browser Engine is unavailable and inactive custody remains unconfigured.
See [public preview proof](docs/audits/2026-09-11-public-preview.md).

Mac checkpoint Homes retain installed code `3c2f9a80` and Runtime SHA-256
`1a55c86d95f5ca286bb63ee65242f3c23e19d0d1acd29888dc976b0033639666`.
Main/dev and contributor PRs retain their prior identities. Full J1 to J5 and
D1 to D6 acceptance remains open. The [team report](docs/audits/2026-09-11-team-sync.md)
and [contributor review](docs/audits/2026-09-11-contributor-review.md) cover
5 to 11 September. Next implementation: exact Marketplace-to-Assistant handoff
and safe removal, then cold Content/Carrier delivery.

Earlier dated records below retain their original artifacts and verdicts. This
checkpoint owns the current public installation status.

## Installed and source evidence

The September 11 integration now uses the full donor history. The combined source
preserves current recovery, cookie and shutdown code while adding model admission,
provider lifecycle, storage and window behavior. Focused source and rendered
fixtures pass; the earlier recovery human Homes remain `1e320578`. Full model history is
merged by `b318bfda`, and Sash shelf ancestry is reconciled by `37c82d4f`. Irzhy
foundation `617796a9` retains current lifecycle fixes after a reproduced explicit
Carrier bind defect was repaired. Its focused source checks pass; dedicated
provider-host and J5 installed proof remains pending. The bounded installed Qwen checkpoint now passes on Mac. The adapted URUX/UIUX candidate is already included through Irzhy reconstruction
(`8b547590` → `7dd1780b` → `985fdffc`), with later Sash dock and lock-face
changes. The proposed full old-URUX-tip merge is superseded by review of concrete
remaining behavior differences. The remaining protected-content stack retains
its source order and feature-preservation gates
in [the integration check](docs/audits/2026-09-11-integration-preservation.md).
The approved Assistant consolidation `fa297cb5` now passes source, rendered and installed Mac checks: one identity, real Qwen reply/save/reload, original draft/model preservation and full Runtime restart. The separate human Home now has an enrolled passkey account. Human review exposed stale System/Marketplace archive metadata; guard `822c4e3d` and corrected installation receipts close that launch gap. Anders confirms Assistant works. Current candidate `3c2f9a80` adds Keep intent during preparation, ordinary Marketplace model rows/details, shared Models theme tokens and distinct Assistant activity. Both Mac Homes have matching Runtime `1a55c86d95f5ca286bb63ee65242f3c23e19d0d1acd29888dc976b0033639666` and 84 served browser artifact records. Installed System retention readback and a real Qwen reply/save/reload pass; the human Home shows Qwen available locally with account and data preserved. Pending preparation is covered by Runtime/rendered tests while installed proof reuses the existing model admission. Original UI parity beyond this slice and full cross-platform J3 acceptance remain open. See [the convergence check](docs/audits/2026-09-11-assistant-convergence.md).


The earlier isolated Mac J3 receipt records Runtime `fbf1a4b0` with matching build,
installed and served hashes and current manifests. Signed catalog visibility,
Use/cancel/retry, exact local package admission, cold startup, exact model
selection, an actual Qwen reply and its saved conversation pass. Reply and unsent
draft survive reload with the same CID/offer and zero implicit dispatches.
Explicit Stop returns `settlement_unknown`; the UI says “Outcome unknown” and
keeps that result after reload. This passes the approved honest-unknown alternative;
confirmed backend cancellation remains unproven. The native adapter deliberately
preserves this distinction when HTTP stream closure cannot confirm backend stop.

Owned shutdown closes all 29 processes, including the model engine, closes a held
HTTP connection and releases the port. The same installed Home restarts and produces a second real reply. All six package
files keep their hashes, sizes, inodes and modification times; protected files and
prior message hashes match. The original unsent draft is restored. There are zero
Content mutations and zero Bitswap payload bytes received during measured reuse. The two model-menu gaps are repaired by `8f28b6e3` and pass installed checks: zero
stale empty copy, a 10 px gap after refresh, normal picker close and unchanged
workspace after reload. A copied release-cache metadata mismatch was captured
and corrected as a source installation; the stamper regression prevents recurrence.
Human Homes retain their existing candidate. Linux J3 requires additional disk
headroom and a reviewed target startup profile. Its previews and data are preserved.
The [integration check](docs/audits/2026-09-11-integration-preservation.md) records
failures, repairs, independent review and the remaining full J3 gates.

Last updated: 2026-09-11 UTC

- Historical publication `00003b6f`, tree `7d816617`, was the website/recovery
  review checkpoint. The later local integration donor and Assistant/model
  repairs are now included in published integration. The original zero-offer
  and duplicate-Assistant observations are superseded by the bounded installed
  Mac results above; their original diagnostics and receipts remain historical.

- The first J3 bootstrap slice integrates donor `c95cf4c9` with the current
  media setup call sites. Small artifact fixtures and all 56 Runtime setup
  tests pass on Mac ARM64; the basic repository gate passes. This is source
  verification. Installed Homes remain `1e320578`; local provider/admission
  source is now in the full donor merge. The installed diagnostic follows. See the
  [bootstrap report](docs/audits/2026-09-11-j3-bootstrap.md).

- User-review follow-up `1e320578` is installed in the two isolated human
  Homes. System displays the restored Profile name for an unnamed current
  passkey, and Recovery/Advanced spacing is repaired. Mac and Linux stay
  signed in together in one browser cookie jar through reload/refresh;
  signing out of Mac leaves Linux signed in. Current source, artifact,
  process and served-file receipts pass. Restored user data is preserved.
  AUTH-01 awaits final user acceptance. See the checkpoint report below.

- The original combined automated J1/C2 checkpoint passes on Mac ARM64 and Linux
  x86_64 installed candidate `f21d285e`. Browser recovery restores the original
  Profile DID/name and covers wrong-password data preservation, retry and
  reload/sign-in/System continuation. `a45164cf` repairs completed-kit retry
  after a new sign-in; `f21d285e` repairs managed child ownership at shutdown.
  Both targets close three held connections, all owned descendants and their
  coordinates, release the port and restart the same Home. Full Home setup
  repetition, media integrity and installed Carrier cleanup/reuse pass.
  [The report](docs/audits/2026-09-11-first-checkpoint.md) binds source, hashes,
  receipts and evidence limits. AUTH-01 retains its pending human passkey
  verdict; isolated Homes are prepared. Automated test services are stopped,
  C3/J3 execution has resumed and recurring monitoring stays paused. Existing user previews
  and public stage retain `d790a48e`; public live remains unchanged.

- J1 identity foundation integrates registration RP/origin binding and atomic,
  conflict-safe identity persistence from donors `145fec2b` and `eb25f747`.
  Forty-seven identity tests, eighteen gateway registration tests and one
  cross-process auth-lock test pass on macOS ARM64 with Rust 1.91.0. Basic
  repository gates pass. Existing gateway behavior is preserved. This is source
  acceptance; installed J1 proof follows the remaining setup changes.

- J1 account setup binds Create/Recover intent to passkey enrollment and
  resumes interrupted owner creation. Product candidate `d790a48e` passed real
  signed Mac installation, default setup, account creation, sign-out/sign-in,
  System entry and Profile-inclusive Recovery Kit export. The user also ran
  the installer successfully. Full recovery and the app matrix remain open.

- The private seed now serves `d790a48e`, with Runtime SHA-256
  `6068849b898284465980b049f37488f2f7de95db15c981529eadb389d7c41a53`.
  Source, installed and served artifact checks pass. Actual passkey step-up
  and a 5,032-byte Recovery Kit export pass; the kit includes the Profile and
  matching principal binding. Smoke repair `49a62db1` selects the foreground
  System window after recovery controls load. A separate clean preview is
  available. Public live remains unchanged.

- Mac testing exposed unclosed download connections, slow gateway shutdown
  and duplicate media-tool installation. Carrier repair `4e62b6db` passes six
  focused lifecycle tests and basic repository gates. Gateway connection closure passes five isolated tests; media setup reuse
  passes seven setup/cache checks. The combined installed checkpoint now has the scoped results above; full
  journey acceptance remains pending.
  Recovery source `67572db1` enters kit selection before passkey setup;
  exact Profile DID/name restoration, Home summary and retry checks pass. The installed preview retains its earlier recovery flow.

- Closeout user findings: two Home assistant surfaces (one reported outdated),
  model loading failure and Browser loading failure. Both `assistant` and
  `home-agent` capsules exist in this candidate; their intended roles and
  installed launch paths need reconciliation before retiring either surface.
  These are priority open J3/J4 product issues, not verified capabilities.
- Refreshed origin still has dev at `6c61c990`. The protected-content follow-up
  branch moved from `decab1f5` to `06179578`; re-review its current PR62 scope
  rather than relying on the older plan snapshot. Closeout inventory found
  75 local branches and 12 worktrees. The follow-up review removed five fully
  preserved branch names, leaving 70 branches and all 12 worktrees. All removed
  tips and reflog revisions remain reachable; 24 other merged-tip candidates
  retain unreviewed historical reflog work. Dirty donor work is preserved; the detached
  Browser build is reachable from `fix/browser-maturity`. Repository-wide
  branch consolidation remains open.

This file records public-safe current truth for released 0.7.0 and active
development work. Private operator paths, credentials, target identities, and
volatile proof logs remain outside the repository.

## Execution evidence history — 2026-09-10

The closeout facts above and the current handover control the next action.
The snapshots below retain their original source and installed identities;
earlier candidates and pending steps describe the state at that point.

The first execution pilot is the public storefront and `/home/` on the seed,
using reviewed source from the active local work. Mission R2 corrects an initial
private-Mac interpretation. [TASKS.md](TASKS.md) is the current queue; the linked
approved plan retains full release acceptance.

- Freshly fetched public main is `8ac18bec` (`v0.7.0`); development is
  `6c61c990b9a1d0f2c1b00ee6f5f091d3354cafac`, tree
  `184b7a52f93403956817cdb1a8a15781b76bb4e2`. PR58, PR52 and PR54 are merged.
- The user chose that development base for `feat/0.7.1-website-execution`.
  The branch began at the same commit/tree, zero ahead and zero behind. The
  website, route and restart-helper source is reviewed in `cdcb7902`, tree
  `564c5dddeebbd4f963182c05a240a157f6d1a775`, one commit after that fetched
  development ref. This is the scoped C1 source; journey donor intake follows
  under C2–C5. Publication and seed deployment remain pending.
- Follow-up source commits are `92d1a31f` for fresh Linux setup/restart,
  `e3478890` for installer platform/process handling and `c97e1c67` for dated
  hosted evidence. Their file hashes match the reviewed test receipts.
- Local integration donor `6972e165` is clean, tree `8b32a72a`, 51 ahead and
  zero behind dev. Browser donor `50196355`, tree `f650c4dd`, is 164 ahead
  and two behind dev, with seven preserved dirty files at the review snapshot.
  These local lines are absent from the fetched origin branches.
- Recorded Home installation remains `94ed0dc6`; diagnostic source `05ee824d`
  was built from `e9b9baf7` and is uninstalled. Browser recorded Mac Runtime
  is `690170bc`, Linux consumer/Exit is `b8c78d79`, and `de0a299e` remains
  a built candidate. This task has not yet repeated their installed proof.
- Public reads show root 200, `/home/` 404 and `/apps/home/` 200. Published
  installer metadata remains `0.1.2`, with Linux x86_64 and aarch64 entries.
  Served website and Home source identities remain unproved. The site must
  distinguish download, source and hosted Runtime facts.
- PR60 `617796a9` and PR59 `25ab205e` remain open; draft PR62 `decab1f5`
  contains planned follow-up work. PR63 `424acd3d` supplies verified framing
  under D6; the J1 replacement owns closure once visible.
- The source pilot passes 31 focused Rust cases, four manifest follow-up cases,
  Home recovery/sign-out checks, 20 website tests, six website checks and the
  required entropy/format gates. Four rendered desktop/mobile cases cover
  metadata success, absence and disagreement, keyboard use and layout.
  Independent review resolved moving-source instructions, stale installer proof
  and installed-app identity. Full Mac and Linux restart smokes also pass;
  committed objects match the corresponding source and workbook receipts.
  The three new journey rows retain pending target acceptance.
- The installer bootstrap passes 18 offline tests with stock Bash 3.2,
  including signed publisher envelopes and RFC 8032 vectors. Independent
  comparison with a second Ed25519 implementation accepted 128 valid signatures
  and rejected 256 modified inputs. Canonical Mac data/process handling and
  complete candidate installation remain open at that bootstrap snapshot.
  The subsequent platform slice passes 38 tests covering canonical Mac/Linux
  data paths and process ownership. It preserves another installation sharing
  a binary and retains state when captured children survive shutdown. Independent
  review passed. Real three-platform installation remains open.
- The 30-minute monitor is paused at closeout. Historical monitoring proof: Nine independent process cases
  passed review. Its first real observation exposed task permission limits;
  a writable shared state and coordinator reads resolved them in a second real
  observation. The recurring timer subsequently delivered and the monitor wrote
  and verified its observation. Old tasks retain their pause.
- Seed preparation reproduced W2's two Linux prerequisites: an `already_ready`
  upgrade creates no backup directory, and inherited shared-write permissions
  can make the installation unsafe. The repairs pass focused Mac/Linux tests,
  including empty and malformed receipts, inherited permissions and preserved
  existing artifacts. The repaired frozen source `3e27cac8`, tree
  `619657c5fee77bb226d5dedff689f873cbfb93e6`, completes fresh target
  installation and startup using the completed build cache. Built, installed
  and running Runtime SHA-256 is
  `cac06eaef00b95559a5415ddce2ef4c3ba428d9c47deb34eb13121e2624e8947`.
  All 25 HTTP checks pass, including website/Home artifact parity, redirects,
  manifest identity and traversal rejection. Private desktop/mobile layout,
  passkey sign-out/sign-in, Recovery Kit download and Profile creation pass.
  The public target package is prepared. Public deployment and full journey
  acceptance remain open.
- Hosted website evidence can now identify a dated operator check of source,
  tree and artifact hashes on the exact serving origin. Its source template
  stays unverified until public proof. The follow-up passes 22 website tests,
  six truth checks and four rendered receipt cases; it keeps install actions
  unavailable. This dated record makes no claim of continuous verification.

- Subsequent delivery source `a0743a17` binds the exact release envelope bytes
  to a digest in the signed head. Independent review, 42 installer tests and
  ten updater tests pass across both metadata transports. This source is outside
  frozen seed `3e27cac8`. New clients require binding-ready publisher metadata;
  publication and rollback must preserve that ordering.
- Installed browser QA exposed stale driver controls and expired-token cleanup.
  The repaired driver preserves recoverable test credentials, checks the actual
  cleared sandbox and uses the visible returning-user control. Independent
  review accepts the repair. Failed attempts remain in private evidence.
  Ordinary seed UI checks then completed Recovery Kit and Profile creation.
  A subsequent full app smoke stops at its outdated System launcher selector;
  automated shell/app-matrix acceptance remains open in J1/C2.
  The driver now uses the source-defined ElastOS menu and System Settings
  control. Syntax, entropy and format checks pass; target rerun is pending.
- The monitor delivered recurring observations but misapplied a historical stop
  from a prior mission. The original message date and scope established the
  error; the correction was withdrawn and the freshness rule is now explicit.
- The user reported 20% weekly usage before the first public result. Current
  execution is focused on C1 public website/Home delivery. Broader installer
  and journey work stays queued at this boundary.
- The public `v0.7.0` tag is `8ac18bec`; the distinct local `0.7.0` tag is
  `3585f340`. The old updater selects release platform by CPU architecture,
  so its Mac first hop needs an actual baseline and a recorded bridge decision.
  Current tag assets and source-home receipts alone do not establish the
  required ordinary update path.

- The storefront follows the reviewed design reference with platform tabs and
  a centered introduction. It explains local AI, Browser and protected publishing,
  provides a first-use guide, describes Runtime/Home/Apps/permissions, and shows
  feature availability. The 0.7.1 command remains hidden with Copy disabled until
  its installer is served and verified; old public-version copy is omitted.
  Desktop/mobile, keyboard and clipboard/fallback checks pass. User review and
  public deployment approval remain open.
- The candidate installer now runs Home setup and opens terminal Home after
  verified bootstrap. It uses the installed binary's absolute path and reads
  interactive input from the terminal. Headless setup prints the launch path;
  install-only mode lets automation own setup and process cleanup. Forty-three
  offline installer checks pass; four optional captured-public-fixture checks
  were skipped. This is source/fixture proof. Actual fresh-device installation
  and browser Home launch remain open.
- The installation guides now explain that the source Home profile already
  includes Documents, Library and the CLI's IPFS backend. Getting started omits
  redundant component setup. The Mac guide selects an explicit development ref,
  loads Rust into the shell and supplies the required collaboration setup mode.
  These documentation corrections preserve the open Content/Carrier acceptance.

- Release publisher manifests now use full OS/CPU platform filenames in local
  exports and ledger reads. A regression reproduced Linux ARM capsule data in
  the Darwin ARM ledger before the repair. The three-platform ledger test and
  all 21 publisher tests pass; shell export checks pass for three host/cross
  combinations. Independent source review found no further affected consumers.
  Native support assets now select the host OS target while microVM guests
  retain Linux targets. ARM Linux preflight now checks the musl artifact used
  by the shell publisher; GNU-only input rejects before publication. Three
  native/guest target cases and all 21 publisher tests pass with independent
  source review. Complete artifact assembly and fresh installation remain
  delivery work.

- The publisher now assembles its served artifact directory before release
  signing, including universal app archives previously omitted from that copy.
  Staged Runtime and manifest hashes must match the release descriptors. The
  component checker also verifies advertised local app, provider metadata and
  capsule file sizes/hashes, and rejects unsafe paths and file types. Five
  regression groups pass, including ten cases through the actual pre-signing
  gate. Independent source review found no concrete issues. These checks are
  wired into CI and `just verify`; remote CI has not run for this local work.
  URL-only dependencies, complete platform-input admission, atomic promotion
  and fresh installation remain separate open delivery checks.

- The low-level publisher now rejects `--dry-run` before side effects and
  directs operators to the existing read-only `elastos publish-release` planner.
  The former shell mode still reached upload/Publisher-write code despite its
  no-write description. The full-script tripwire test reproduced an attempted
  temporary-key allocation, then passed after repair. Six publisher regression
  groups and independent source review pass. Normal publication keeps its
  existing behavior and approval gate.

- App and provider-metadata archives now use Python's standard library, replacing
  GNU-only tar options that failed on macOS. The archive writer preserves app
  layout, executable files and symbolic links, with normalized file metadata.
  Seven publisher regression groups pass. The same fixture on macOS ARM64
  (Python 3.9) and Linux x86_64 (Python 3.12) produces identical compressed
  bytes and extracts with each system's tar. Independent source review passes.
  Archive hashes change from the previous writer; a new candidate must bind
  the new bytes. Full candidate builds and device installation remain open.

- Direct app, native provider and provider-metadata builders now record local
  file descriptors before any capsule upload. Publication attaches real CIDs
  in a separate step. Build and archive failures return through Bash command
  substitutions, and nested app entrypoints get their parent directory before
  copying. Ten regression groups pass on macOS and Linux, including four
  preparation failures that stop before upload. Independent source review and
  required source checks pass. Actual candidate installation remains open.

- The native preparation worker builds from a clean Git revision and tracked
  Cargo lockfiles, then exports local artifacts with an unsigned source/file
  receipt. Admission checks all three platform inputs against the candidate
  source, component template, native OS/CPU, provider contracts and exact file
  inventory. Preparation and admission fixtures pass for all three platforms.
  Actual Mac, Linux x86 and Linux ARM preparation pass from 8587dff6, including managed
  media delivery. All three inputs pass full file verification; shared app and
  provider-metadata archives match across platforms. Linux ioctl and custody
  syscall repairs pass native compilation and focused tests. Publisher input
  staging and its CLI pass offline checks and Linux musl tests. Complete
  publication promotion, final input regeneration and signed fresh installs
  remain open.

- Three standalone Linux Browser helpers now have tracked Cargo lockfiles.
  Their registry versions and checksums match the existing Runtime workspace
  pins. Offline locked dependency resolution passes for all three projects.

- Home CLI delivery now uses one platform-specific capsule archive containing
  its native terminal renderer. Runtime and source-home provisioning use the
  same installed capsule path. Checks reject missing, non-executable, malformed
  or wrong-platform renderers before launch. A real Mac renderer build and
  archive extraction match byte for byte; narrow Rust extraction/launch tests
  and eight preparation fixtures pass. Development fixtures use the installed
  renderer, and the demo reports incompatible older releases before launch.
  Full signed fresh-device acceptance remains open.

- Managed Home now has one media-tools component containing FFmpeg and FFprobe,
  built from pinned FFmpeg 9.0.1 and x264 sources with their source and licenses.
  Setup installs and verifies this archive before private media import. Ordinary
  setup uses the supplied pair; source-home has an explicit developer directory.
  New data roots and archive directories have explicit permissions. Tests cover
  common umasks, real archive extraction/import, existing unsafe paths, pair
  mismatch and source/recipe tampering. Mac tools build with system-only shared
  dependencies. Signed fresh installation and replacement of older imported
  media tools remain open.

- The focused required video repair from donor e3a8c4eb makes media-provider
  accept Runtime's current invocation ABI and normalize FFmpeg DASH segments to
  the existing zero-based contract. Runtime preserves codec configuration boxes
  through protection. Provider and contract regressions pass; an actual generated
  video run through the relocated tools, provider and Runtime validator accepts
  one track, two segments and 128 samples. Full J5 purchase/playback/cleanup and
  the required external review remain open.

The source and installed sections below preserve the earlier evidence snapshot.
Their original identities qualify those results. Current milestone receipts
above take precedence where the work has moved; remaining sections are refreshed
as their journeys are accepted.

## Prior release/source snapshot — 2026-09-03

- A fresh fetch records `origin/main` at `8ac18bec` as the released `v0.7.0`
  source and `origin/upstream/0.7.1-dev` at `c511b133` as the active
  integration line.
- Released `v0.7.0` already carries the coordinated workspace version,
  changelog, manifest bumps, and lock refresh. Installed artifacts report
  `0.7.0` only after the checked publish flow stamps
  `ELASTOS_RELEASE_VERSION`; unstamped source builds report `0.7.0-dev`.
- Local candidate `900d7e5c` has tree `c9a9effe` and is 30 commits ahead of
  `origin/upstream/0.7.1-dev@c511b133`. It contains the reviewed PR52 source at
  `origin/feat/protected-content-installed-provisioning@4d688cc5`, PR54 at
  `origin/feat/home-first-run-seed-0.7.1@2a49ea57`, and the PR55 Home Agent
  source from `origin/feat/home-shelf-assistant-face-0.7.1@923193bb`. PR54 and
  PR55 remain the original feature review slices. The tested candidate still
  needs publication and combined team review before a merge decision.
- The published protected-content stack is contracts `0c56c56a`, custody
  `2f844cef`, key reconstruction `467a6c03`, custody provider `1b7fa732`,
  Wallet rights `c9e82e75`, Runtime `a8ac6dc8`, and rights `3627da01`.
  Every tip is an ancestor of the published lifecycle and is already present
  in the active integration. The latest published protected-branch repairs
  need no new extraction.
- `main` and `origin/upstream/0.7.1-dev` already include the reviewed Home
  audit fixes, the named principal-root write policy, checkout-bound test
  fixtures, the privacy-reviewed audit workbook, the completed-mint adoption
  repair from `58ebfb23`, and the equivalent CPU watcher optimization at
  `8e53174f`. The reviewed donor `e4d897f6` is the source comparison, not an
  ancestor. The candidate source passes formatting, alignment, Home and Browser
  entropy, Home shell, People discovery, the 26-case Browser close handshake,
  and Home Agent shell checks. Remote CI remains separate from these local
  checks.
- `origin/upstream/0.7.1-dev` also carries Irzhy's verified Base 8453 probe
  evidence, shared build-artifact staging, upstream collaboration work, and
  Browser local-exit orphan cleanup. The protected-content source path remains
  inactive. Installed proof on isolated localhost, the seed and third custody
  node, and one atomic cutover remain open.
- Commits `3026992b`, `ed7a8bfc`, and `7f6e47f9` provide portable listing
  publication and import, buyer purchase, and buyer open, read, and close
  without creator Runtime mint state. The package binds the public custody
  identity to Chain-committed metadata and uses one immutable listing
  projection on each Runtime.
- Commits `ba7f6cea` and `84569da5` complete exact buyer Runtime rights admission
  and the two-Runtime source journey. Runtime A keeps provisioning authority for
  the real process-backed 2-of-3 custody nodes. Runtime B imports the listing,
  buys it, and completes open, read, and close with its own Profile, Wallet,
  device identity, state, and signed release operation.
- Playback reconstructs from the authenticated release operation, verified
  signed epoch, released contributions and terminal receipt, recipient
  possession, and public CEK commitment. Provisioning still uses
  `CustodyEnvelopeV1`; Runtime stores no playback copy of the custody envelope.
- The combined protected-content gateway proof uses private Runtime targets for
  protect, media, custody, and decrypt. Carrier supplies authenticated endpoint
  transport before the Runtime-selected custody target handles the request.
  Public provider projection excludes these targets.

## Browser contract and device qualification

The Browser mission is paused at the user's request on 2026-09-09. All B01-B16
full acceptance gates remain open. The accepted contract permits dependent work
to continue; B01's open support matrix does not block every implementation slice.
[Acceptance requirements](docs/BROWSER_ACCEPTANCE.md),
[support matrix](docs/BROWSER_SUPPORT.md), and the
[current J4 handoff](TASKS.md#now) and
[B01–B16 controls](TASKS.md#browser-maturity-workstream) remain canonical.

The task Mac and Linux consumer/Exit have these installed artifacts:

| Artifact | Verified source or SHA-256 |
| --- | --- |
| Mac Runtime, source `690170bc` | `f5b012ae4f704bf22cb94aa51699f29082839727b4ddbd4493b7b14d26c557bf` |
| Mac Engine adapter | `192139afe6b274a25bc258648aa84cb98309b708ad3a7d86996ba9095be93c57` |
| Mac rootfs | `58c82e397fa97d900159ba1a0a7854505cb1bce1f5e11a2c2a8e40e5a7f03971` |
| Mac initrd | `d928d9f69e7929049575cb1f4c6366f87f66c62cf11921faddb5bb2963c5462c` |
| Linux Runtime, source `b8c78d79` | `47ca650f8033727bfe57c20b52d4392fdb52d2f085b60307a5394697c44a87a3` |
| Linux Engine adapter | `83b59f996fae053b66bfaf9a88111c77e0c8846966335d6724d4462a8c0c52c7` |
| Browser UI on both installations | `f8515b7a28d8bd3dd191ce21ea5c6d1b8c7ed2eadf69852c8cdfd5128fee7ea0` |
| WebRTC viewer on both installations | `f42217e02d39983924ab9311633c6a91696d8f8eb635317c3ffe7d185944a6e7` |

Installed, served and provider checks pass for these sets. The Linux update
preserves all five task configuration hashes. Native Runtime builds are debug
builds, and the Mac image includes a bounded audio observer. They establish
functional results; release performance still needs an exact release candidate.
The kernel, media dependencies and matching helpers were reused wherever their
inputs stayed unchanged. The final source-only patches below are not installed.

Independent review accepts these bounded installed milestones:

| Journey | Passed | Remaining limitation |
| --- | --- | --- |
| Local 92 | Home launch, controlled navigation, decoded video/audio, typing, scrolling, Engine inspection, viewer reload in 3872 ms, all 13 close effects in 788 ms, final Runtime/control counts zero | Individual functional sample; observed dropped frames and cumulative audio loss do not establish quality or latency distributions. |
| Operator 93 | Actual installed Camofox and unchanged Playwright SDK workflow: owner invitation/approval, inspection, fill/clear, revoked-writer rejection and detach preserving the owner's page; media, reload and all 13 close effects also pass | Bounded adapter surface. General selectors, waits, actionability, frames, files, headless handoff and declared Camoufox/Firefox conformance remain open. |
| Remote 96, A/B/A | Linux Home/consumer and Exit with Mac Engine: approved selection, controlled page, decoded frames, navigation, typing, inspection, short audio and scrolling; exact close in 1888 ms, all 13 effects absent and Runtime/control counts zero | Integrated run fails viewer reload at the unchanged five-second gate. Completed Runtime samples retain page binding; fresh viewer media is unproved before the deadline. |
| Earlier A/A/B | Mac consumer/Engine uses approved Linux Exit for a public page and navigation; denial and renewal propagate; exact close reaches zero obligations | Controlled remote audio/input, pre-allocation revocation behavior and all placements still need proof on the final candidate. |

Remote 96 first fails at `state_deadline` in the reload observation. Viewer
page-status takes 1019 ms and precedes display attachment, which starts 3464 ms
after reload. The observation also reads summary, remote status and local media
serially; its failed substep was not recorded. A frozen UI scheduling patch
starts status and attachment together after retained-owner admission, and a
separate observation change records bounded substep timing. Source verification
passes 302 focused tests, including an old-order regression failure, and
independent review accepts the bounded slice. The dedicated delayed-JSON
observer test and installed repeat remain pending. Existing authority checks
and the five-second criterion remain.

Earlier source and installed work provides Engine 2.1 readiness, image-set
verification, bounded image acquisition, explicit Engine/Exit selection,
Runtime-owned launch/close settlement, input and restored-address repairs,
WebRTC attachment/recovery, Engine-page inspection and scoped operator writes.
The RNG initrd change removes an observed five-second bootstrap delay; measured
launcher samples still range around 9-11 seconds. These are accepted contract
or bounded journey milestones, not completion of their full acceptance areas.
A real roughly 898 MB image package is verified locally. Ordinary automatic
acquisition of a complete published compatible set and second-maintainer setup
remain open; source-home preparation still uses operator-supplied artifacts.

| Additional source work | Evidence and installed status |
| --- | --- |
| `a3471808`, directional Exit EOF | Original regression fails; both repaired stream-direction tests pass independent review. Installed in Linux `b8c78d79`. Remote 96 advances past the prior navigation stall, but one pass does not establish sole cause or repeatability. |
| `de0a299e`, auth renewal and audit in one state mutation | Independent review and 24 actual Rust checks pass, including six-to-three validation passes. A Mac Runtime candidate built successfully; installation and measured Home attribution remain pending. |
| `52238f2f`, preserve existing profile disks after mount failure | Original destructive path fails its regression; 14 generated guest-shell and four VZ tests pass independent review. Install the matching host and guest together before persistence proof. Linux Browser profile disk attachment still needs implementation. |
| `b8c78d79`, paired profile fixture/harness | Reviewed write/close/read proof for cookies, local storage and committed IndexedDB. Attempt 94 fails Home startup before Browser allocation; paired read 95 was stopped. Actual persistence remains unverified. |
| `6ff70451`, selected upload bytes | Reviewed fixture validates an actual 64 KiB file and destination hash. Installed Library upload and changed-byte negative case remain pending. |
| Frozen update candidate validation patch | Validates the staged executable before replacing the working Runtime. Three real-subprocess regression fixtures are prepared; actual Rust red/green execution and installed update proof remain pending. |

Failures remain explicit. Earlier remote attempts include an unexplained Carrier
connection failure and navigation timeouts with incomplete close receipts.
Audio runs 57 and 72 and lifecycle probe 02 fail with silence or loss; later
short passing probes do not explain them. A 60-second idle observation consumes
34.4 CPU seconds with zero pages/VMs. Collaboration validation appears in sampled
stacks, while a separate Home trace identifies auth-lock contention and repeated
audit validation. These observations do not establish the full idle-CPU cause.
Sash's failure on his own installation has no matching diagnostic receipt yet.

The observed Mac is ARM64 with 24 GiB RAM and macOS 26.5.2. The Linux AMD64
server supplies consumer/Exit evidence and has no KVM device. Every support
matrix row retains full device, media, recovery and human qualification.
Required 100 cold launches, 100 warm launches, 100 lifecycle cycles,
uninterrupted 30-minute A/V and eight-hour mixed use have not started. The
planned soak start was missed. The current runner lacks warm conditioning,
input-to-visible latency and synchronized A/V offset proof. Full completion
within the original deadline is unsupported.

The final Home entropy, Browser entropy, display-mode and formatting checks
pass. The objective audit returns failure because accepted provider media and
matching manual UX evidence are absent. Two older documentation predicates also
fail; they already failed before this cleanup and need reconciliation with the
current Runtime contract. This audit establishes no product-readiness result.

Runtime continues to own host compatibility, service selection, capabilities,
lifecycle and audit. Providers own rendering, page semantics and egress;
Carrier transports authorized remote effects. Human and agent operations share
page authority. Profile protection/transfer, complete daily workflows and Wallet,
human accessibility, bounded leases/revocation, updates and security maintenance
retain their requirements. Public live and published source are unchanged by
this mission. Earlier per-run detail is retained in Git history and private
hash-bound receipts rather than duplicated as current truth.

## Branch Hygiene

- Local UIUX subgroup branches are extraction scaffolding already contained in
  the published UIUX candidate and active integration. They need no separate
  publication.
- Local accepted protected-content labels whose tips are ancestors of the
  published lifecycle or upstream need no separate publication.
- Retained donor branches and dirty worktrees remain under the operator
  ledger's preservation rules. The August review carried useful content,
  Recovery/Profile, Windows and operator documentation into this candidate.
  Older Assistant and migration donors retain explicit deferred tasks.
  Preserve unique history and original dirty files until their owners approve
  cleanup; published source does not make every older hunk equivalent.

## Installed candidate proof

- The isolated localhost installation at `localhost:61380` has source commit
  `900d7e5c` and tree `c9a9effe`. This is installed acceptance for that local
  candidate. It is not seed, third-node, or cutover proof.
- The manual Brave journey opened and inspected System, Home launcher and
  windows, Profile and People, Chat, Inbox, Wallet, Marketplace, Services,
  Library, Archive, Documents, Player, standalone Assistant, Home Agent, GBA
  Emulator, Nonogram Advance, uCity, and Browser. Home fullscreen fills the
  workspace and returns to the saved layout.
- Both games render and their expected controls work. Browser opened
  `ela.city` through the VZ adapter and closed with no per-launch supervisor or
  TURN server residue. Startup took about 34.5 seconds. Transient TURN
  authentication failures occurred before allocation succeeded, so startup
  latency and those diagnostics need follow-up.
- Home Agent opens from the desktop and from Ask Assistant into the Home-owned
  Agent Space. The installation has no configured model offer and reports that
  state directly. This does not prove model inference.
- People reports discovery as unavailable because the isolated Home has no
  collaboration configuration. The pending Wallet approval remained unchanged
  during the journey.
- Protected-content acceptance is blocked on one real three-node custody
  composition, private Chain and RPC configuration, three replicas, funded
  creator and buyer accounts, and installed two-Runtime proof. No mint, buy,
  playback, seed, third-node, or cutover claim follows from this localhost run.
- First-run work remains open. Recovery Kit navigation and readiness are slow
  and indirect. Profile creation should carry the display name into the form
  and guide the exact create action.
- Standalone Assistant and Home Agent are both installed pending an explicit
  product decision. The final default product should present one clear Agent
  surface. Multiple local Brave app copies can create duplicate Dock and
  recent-app entries; this is local operator hygiene, not product architecture.

## Integrated Source Truth

Runtime retains `tracing`. Irzhy postponed the replacement `elastos-logger`
on August 30; its absence is an intentional decision, not an omitted release
feature. Its useful VM-payload privacy repair is included independently in
`74ed3bc9` and has a log-capture regression test.

The older July Carrier branch contains framing, deadline and protocol work
that needs an adapted integration: the current incoming request handler still
has an unbounded line read, while the donor's whole protocol would disable
provider invocation used by the current protected-content path. This remains
an explicit source-integration decision in
[deferred work](docs/DEFERRED_WORK.md#retained-source-integration).
Inclusion of all retained work requires a behavior-level comparison, not only
commit counts.

Older Assistant attachment, knowledge/search/citation and advanced Studio
implementations remain retained donors for the open work in
[deferred work](docs/DEFERRED_WORK.md#retained-source-integration).
The PR15 legacy-auth migration also remains separate because it replaces
unchained audit history. Current signed-checkpoint policy owns compatibility;
retaining those donors does not mean their behavior is in the candidate.

The reviewed content-distribution, Recovery/Profile and WSL-first documents
are included. The combined source projects installed capsules and a verified signed model
catalog. Local Content preparation and admission are implemented; ordinary cold
Carrier delivery and the Marketplace-to-Assistant handoff passed for SmolLM2
on the signed-in Mac Home. Full Qwen and broader installed acceptance remain open. WSL
packaging and native Windows support also remain unproved product targets.

Runtime owns authenticated principal and session authority, capability
admission, provider selection, lifecycle, durable operation identity, Wallet
and Chain coordination, audit, and settlement. Providers own their typed
operation semantics. Carrier transports only Runtime-selected endpoint traffic.
Capsules own presentation and app behavior and receive bounded read models and
opaque selectors only.

The integrated source includes these durable facts:

- Collaboration identifies people by Profile DID and uses endpoint DIDs for
  private Runtime routing. People and Chat consume typed Runtime projections.
- Home owns shell chrome, launch framing, focus, fullscreen, clipboard
  mediation, notifications, and sign-out. Capsules own their content and icons.
- Wallet owns accounts, approvals, signatures, and transaction effects. Runtime
  binds protected-content creator and buyer operations to the verified Wallet
  account and configured Chain authority.
- Assistant is a capsule. Runtime selects one model offer through
  `ProviderRegistry`, and `model-provider` owns model execution. Assistant
  renders bounded, sanitized markdown and math from typed output.
- Library prepares, protects, lists, and opens video through typed Runtime
  operations. Marketplace reads immutable listing projections and submits only
  the mint identity for buy or open. `elacity-player` owns video presentation.
- Runtime keeps the canonical protect, media, custody, and protected-content
  decrypt providers on reserved private targets. Public capsule lookup,
  interface projection, and provider routes exclude these targets.
- Protected publication requests exactly three replicas. The repair task keeps
  the same requirement. Purchase and open both require fresh signed
  availability bound to the exact protected object.
- Protected Chain configuration is an owner-only
  `protected-content/chain-provider.json` file. It contains one versioned
  `protected_content_network`; Runtime supplies its operation issuer
  separately. Node-local rights evaluation uses 2-5 explicit private RPC
  sources and requires two exact agreeing finalized results.
- `scripts/protected-content-installed-static-audit.py` reads installed
  artifacts and emits
  `elastos.protected-content.installed-static-audit/v1`. It reports source and
  static artifact failures, operator configuration prerequisites, and active
  installed proof prerequisites separately. `ready_for_active_proof` is a
  static admission result, not product readiness.
- Full `scripts/setup-source-home.sh` installs one stable Runtime at the
  platform data root under `bin/elastos`. It writes the owner-only
  `receipts/source-home-installation.json` receipt after components, native
  providers, capsule trees, and source-home capsule metadata are final. The
  receipt binds source commit/tree/clean state and exact artifact hashes.
  Setup requires at least 10% free space on both source and data volumes before
  builds. Its private install stage is removed on success, copy failure and
  installer failure, as verified by the isolated installation smoke. Source
  setup and Browser target refresh each retain one default VM backup set.
- `scripts/mac-source-home-restart.sh` and
  `scripts/linux-source-home-restart.sh` select only that stable Runtime.
  Each owns one exact PID file, stops only the identity-bound prior Runtime,
  retains at most one bounded principal-root rollback, and writes an owner-only
  restart receipt. Mac default mode uses the existing installation; `--init`
  also requires current clean source and artifact parity. Mac dry-run validates
  without stopping the Runtime, including with `--down`. Runtime owns provider
  shutdown.
- The macOS replacement-restart path is proven in its fixture on this host,
  including a live prior Runtime after atomic binary replacement. Linux dry-run
  and fixture proof is source evidence; active `/proc`, listener, and binary
  replacement behavior still requires Linux target evidence.

## Protected-content Contract Truth

KID and `EncryptedContentIdentityV1` are separate identities. The bytes16 CENC
KID is the deployed AuthorityGateway access key. The full encrypted-content
identity binds the protected object and media contract.

The active local branch preserves the verified deployed read behavior:

- `AuthorityGateway.hasAccessByContentId(address,bytes16) -> bool` owns the
  access read.
- `CentralStorage.ipReference(bytes16)` resolves the KID for that read.
- An unknown KID reverts with `UnboundContentId(bytes16)`; a bound KID without
  access returns `false`.

`origin/upstream/0.7.1-dev@c511b133` includes Irzhy's deployed Base 8453 probe
evidence from `90bbe15b`, already present in this branch:

- `CentralStorage.bindIP(bytes16,address,uint256)` accepts acknowledged
  contracts only and is called by `AssetFactory.registerNewAsset`.
- Native `AuthorityGateway.buyAccess` uses selector `0xf7580ad9`.
- ERC20 `AuthorityGateway.buyAccess` uses selector `0x0ede2294`; Wallet approval
  targets each operative `paymentProcessor()`.
- EventHub emits mint events.
- Upstream records bound-KID allow, deny, and unbound evidence.

The exact funded buy receipt and event remain installed proof items. Deployed
`View` and `Download` still map to one boolean Chain access result, so signed
Runtime policy owns the action distinction until contract evidence defines it.

The canonical source path keeps Runtime journals limited to identities, state,
receipts, and settlement. Protect, custody, and decrypt providers keep clear
media, ciphertext staging, CEKs, and shares inside their private process
boundaries. Each custody node owns one independent share and its node-local
rights check. Runtime and capsules do not receive private provider, storage,
Chain, RPC, or Carrier topology.

Verified on branch `feat/protected-content-installed-e2e-proof` between
2026-09-02 and 2026-09-04, against the simulation-only
`deploy/custody-host/` three-node harness (see its own README for the
simulation boundary):

- The three-node compose harness is live with DID-keyed public descriptor
  handoff, and node resurrection after stop/start is proven on three separate
  occasions: each node returns with the same DID, a fresh readiness receipt,
  and zero required environment variables.
- Real `CarrierPeerDid` transport dial proofs exist for each node: a signed
  operator-control denial plus the container-side audit log recording the
  dialing client's DID.
- The offline 2-of-3 custody composition ceremony
  (`elastos protected-content-config`) runs over three real node descriptors
  exported by the harness, and its own verify path passes.
- `protected-content-installed-e2e-proof.sh --phase preflight` reports
  `preflight_ok: true` for all three nodes' descriptors, dials, and receipts.
- `scripts/custody-harness-ci-smoke.sh` (the CI-safe `provision` + `preflight`
  rehearsal against a fresh throwaway harness instance) passes fully, locally.

Verified on the same branch between 2026-09-05 and 2026-09-07, live, against
the same simulation-only three-node harness, with the installed client
Runtime (`scripts/setup-source-home.sh` receipt, started by
`scripts/mac-source-home-restart.sh`) and three custody-host containers built
from the reviewed server/capsules tree, an Anvil fork of Base as the private
chain (two distinct-origin evidence RPC sources, finality advanced by a block
ticker) and headless, recovery-ready, profiled principals (creator, buyer,
denial) with managed wallet accounts funded through `anvil_setBalance`:

- The full installed journey ran end to end and the finalize receipt reads
  `overall_ok: true` with every required phase present and `ok: true`: mint
  (real ffmpeg DASH preparation, CENC protection, three-node custody
  provisioning, a signed availability receipt with three replicas and a live
  multi-peer proof, the on-chain mint and the ERC-1155 operator approval on
  the operative), availability, buy (fresh availability, `buyAccess`
  finalized on both evidence sources), open (managed-wallet viewer release
  approval, a 2-of-3 release settled by the nodes' own chain rights evidence,
  init and segment reads, close), the custody and replica drills, the
  negative cases (below quorum fails closed, non-purchaser and cross-principal
  reads denied, stale replay rejected, tampered custody share excluded), the
  mid-session restart (SIGKILL between approval and confirmation, no
  duplicate transaction), cleanup (explicit close settles; the boot sweeper
  settles a lease abandoned by a mid-open kill) and finalize.
- Product contracts observed live and now asserted by the driver: one
  stopped or tampered committee member does not deny the viewer (2-of-3
  serves; below quorum with two nodes down fails closed on availability);
  `content status` reports the last stored availability receipt; media parts
  are released strictly in order; a non-purchasing principal is refused by
  the purchase gate before any session gate.
- A custody committee member settles every release through its own chain
  rights evidence, so the standalone provider host and the custody-host
  image carry the chain plane (trusting the provisioned client issuer), and
  each node needs the client's network configuration with evidence RPC URLs
  reachable from the node.
- The receipt's phase blocks cite the branch commits current when each phase
  ran (the branch was re-folded to three commits during the run); the
  binding evidence is the recorded host and per-container binary sha256.
  After the proof, the server/capsules commit took two lint-only edits to
  pass `cargo clippy --all-targets -D warnings` (an explicit
  `too_many_arguments` allow on `RuntimePreparedRecipient::from_persisted_parts`
  and boxing the signed arm of `RuntimeReleaseWalletOutcome`) plus test-only
  changes (the mock wallet binds outcomes on the real six authority fields,
  a regression test for the restart-completion projection); no behaviour
  changed, and the proof binaries predate those edits.

Not yet verified: the same journey on distinct seed/third-node hardware
across genuinely distinct operators and failure domains (gates 3 and 4), a
real Base deployment instead of the Anvil fork, and the in-browser Brave
UIUX path of the journey (gate 8). The local `custody-harness-ci-smoke.sh`
runs are arm64 on Docker Desktop; the CI job `custody-harness-smoke` ran for
the first time on a Linux amd64 runner on 2026-09-07 (PR #57) and failed
before any node exported its descriptor: a bind mount keeps the host
directory's owner and mode on Linux, so the containers' unprivileged user
could not write `shared/` (Docker Desktop maps that ownership away, which
is why no macOS run could see it). `up.sh` and the entrypoint now handle
that host explicitly and the smoke preserves the nodes' logs on failure;
the third run on that runner (2026-09-07, after the same steps had passed on
a plain Linux Docker Engine in a VM) is green: three distinct DID-keyed
descriptors, composition generated and verified, three Carrier dial proofs,
provision and preflight `ok: true`, clean teardown.

## PR15 Extraction Ledger

PR #15 / `feat/dkms-esp-port` is source evidence, not a merge target. The
integrated source adapted these useful parts to the typed Runtime path:

- `6d2e9083`: player/viewer behavior and Library-open UX now use typed Runtime
  launch, read, and close operations.
- `c5aed9db`: Creator UX now appears as the Library protect-and-list flow.
- `57974479`: the grant journey maps to current Profile, session, Wallet, and
  Runtime authority.
- `ffea5998`: useful Create, mint, and open failure cases are covered by the
  current typed paths.
- `e148218b`: applicable CI lessons remain in the focused source and platform
  gates.

Current video opens in `elacity-player`. Document and 3D viewers remain later
typed-viewer scope. External cryptographic review remains open before public
dKMS or production confidentiality claims. Global listing discovery and public
custody governance remain later work. The shared listing link, portable import,
buyer Runtime rights admission, and exact two-Runtime 2-of-3 source journey are
complete. Installed proof and the atomic authority cutover remain open.

## Capsule Execution Truth

- [docs/CAPSULE_MODEL.md](docs/CAPSULE_MODEL.md#isolation-boundary)
  defines the standing cross-branch isolated-execution contract, not proof that
  every first-party app is already a Component.
- The ESP branch proves a useful substrate slice: the Component runner and
  conformance fixture use no linked WASI, environment, filesystem preopen,
  FIFO, raw socket, or gateway authority, and every guest effect is linked
  through `elastos:bus@v1`. This is contract proof, not first-party product-App
  adoption.
- The Component runner is bounded today, but not yet by each manifest's declared
  resources: it uses a fixed 128 MiB memory ceiling and fixed fuel budget.
  `component/v1` is a bounded activation contract and cannot cancel or stop an
  activation already running.
- Component identity context is not complete: principal may be launch-bound,
  but the current Bus host reports the capsule id as the session id and has no
  device binding. Principal, proof binding, device, capsule, launch grant, and
  session therefore are not yet independently proven end to end.
- Whole-bundle publisher verification, signed interface compatibility,
  complete WebSpace state portability, and cross-node historical
  re-instantiation remain open. A valid manifest or checked-in Component is not
  yet proof of an independently durable Digital Capsule.
- Browser and Home web surfaces remain host projections and adapters. Their
  routes, frames, cookies, and placement are not the capsule ABI or authority,
  and the repo must not claim that every visible first-party app is already a
  self-contained executable Component.
- Verified on `b07160cf` on 2026-08-16: the generic
  `POST /api/provider/:scheme/:op` route remains a live host adapter used by
  current web projections and control surfaces. It is not the Component ABI or
  a capsule contract, and new capsule code must not treat it as one.
- Runtime auth audit history is currently SHA-256-linked and Ed25519-signed with
  retained-chain anchoring. BLAKE3 is not the canonical audit hash in this
  branch; any future algorithm change requires an explicit schema version,
  algorithm identifier, golden vectors, and a signed transition anchor rather
  than rewriting retained history.

## Authority And Wallet Truth

- Home authority uses signed `elastos.home.launch-token/v4` envelopes. Runtime
  validation binds resource, actors, principal, proof, grant, session,
  lifetime, and non-delegatability; callers cannot supply Wallet authority.
- Wallet Bus v2.3 is the typed Runtime/Wallet Provider boundary. Wallet Provider
  owns keys, accounts, proofs, approval execution, and validated outcomes;
  Runtime owns launch authorization, orchestration, durable effects, and the
  private provider adapter.
- Passkey step-up is durable, one-shot, and bound to the original launch,
  operation, and canonical request digest. Managed recovery verifies the exact
  Wallet set and root reassignment before returning terminal success.
- Runtime creates durable transaction-effect state before dispatch and
  reconciles recorded or uncertain Chain outcomes without rebroadcasting.
- Browser account access is an explicit Wallet approval. The injected provider
  can request accounts, but the page receives no selected address until the
  Runtime-mediated request is approved through the trusted review path.

## Consequence-aware effect truth

- The manifest schema includes `AffordanceRisk::Actuator`, and Runtime maps it
  to the `execute` capability action. The generic catalog invocation path still
  rejects actuator, payment, rights, and privileged affordances because its
  explicit user-approval dispatch is not enabled.
- No shipped capsule manifest declares `actuator`. This branch has no general
  sensor-observation envelope, installed physical-actuator provider proof, or
  hard real-time safety claim.
- Wallet transactions have durable effect IDs and uncertain-outcome
  reconciliation. Browser launch has bounded `DidNotAct` reconciliation, and
  remote service contracts forbid blind retry after uncertain dispatch. These
  are provider-specific proofs, not a shipped universal effect state machine.
- [Consequence-aware effects](docs/CONSEQUENCE_AWARE_EFFECTS.md) defines the
  shared target contract. Physical effects still require an operation-specific
  provider, destination admission, local interlock proof, truthful settlement,
  and installed-target evidence before any readiness claim.

## Proof Path Ledger

- Installed operator/update proof path: `public-install-operator-smoke.sh`.
- DID/profile proof path: `public-install-identity-smoke.sh` or
  `local-identity-profile-smoke.sh`, depending on target role.
- Public Linux runtime portability proof path:
  `audit-linux-runtime-portability.sh`.
- Provisional protected-content provider retirement guard:
  `protected-content-provider-contract-smoke.sh`. It does not verify the
  canonical v1 custody or Runtime path.

## Browser Truth

- Browser is included as a bounded Runtime Browser, not as a fully reliable
  general-purpose Browser claim.
- Browser launch, TURN/media-relay connection, Runtime-mediated traffic, exact
  terminal close, and zero-residue behavior require fresh target evidence for
  the exact integrated commit. Human-visible video, input, scrolling, and audio
  remain manual proof gates.
- Deterministic proof confirmed `window.ethereum`, one EIP-6963 provider,
  `isElastOS=true`, `isMetaMask=true`, the Runtime Wallet binding, chain `0x14`,
  and exactly one `eth_requestAccounts` handoff producing one pending Wallet
  account-access approval.
- One failed Browser restart followed by a successful open, lost `ela.city`
  login state across restart, and slow performance remain explicit follow-ups.
- Runtime owns Browser launch settlement and exact cleanup obligations. The
  close path acknowledges authority renewal, binds close to the exact Browser
  instance, and keeps nonterminal cleanup ownership durable.
- Browser profile state is principal-owned and reset-scoped, but it still lacks
  protected/recoverable Browser profile storage.
- Browser VM Chromium profile disks are principal-owned and reset-scoped, but they are not protected principal-root envelopes or Recovery Kit-packaged state yet.
- Browser profile receipts must continue to report
  `storage_posture=principal_owned_reset_scoped_unprotected`.
- Principal-root object protection exists for selected Home/runtime state; this does not include Browser VM Chromium profile disks yet.
- Product-readiness claims remain gated on target-specific objective audit and
  matching manual UX evidence; source inclusion does not waive that gate.
- The 0.7.1 Browser U9 journeys remain open. Isolated Mac Home now runs
  Runtime `b6d182b7` (PID 50635). Local U9 evidence stays bound to
  Runtime `63be1c09`, which completed input, TURN SIGSTOP media cut
  (stall 4344 ms, recovery 1810 ms), viewer reload at 1196 ms, official close
  with all 13 terminal effects, and a second open (stall 4018 ms, recovery
  1489 ms, reload 1216 ms). The 22:08 local log remains classified as an
  unproven interruption: CDP offline produced `cut_ms=5000` and `stall_ms=0`.
  The earlier `bd0f4beb` 797 ms local first-frame sample and the remote census
  5340 ms first-frame sample on seed `21388ce6` stay bound to those artifacts.
  Remote full input/reload/recovery/reopen on the current grant is not run:
  the 00:51 seed dump still reports expired offer
  `remote-engine-fdf0eaa2ec5d6ede` and `remote_runtime_binding_required`. The
  vm-control settlement fixture including exit-then-reopen-before-cleanup
  passed. Live page capacity requires the VM to own that exact page. A later
  same-profile VM does not revive an old pending page. Consumer close settles
  already-absent Engine ownership and releases viewer ingress. Source now
  allocates a fresh display-attach request ID after a terminal failure and
  commits preadmitted media only while ingress still owns the page. Those two
  U9 lifecycle races have source regressions. The installed `63be1c09` pair
  still runs the earlier source. Source also closes the model revoke/create
  race and the AI0 Chat/Studio decode plus remote-reply identity boundary.
  The four installed Qwen U8 Busy/retry/restart/denial checks stay open.

## Browser Provider Evidence

- Browser architecture is coherent enough to preserve, but the objective still
  fails product audio proof and hash-bound manual UX evidence.
- Verified on `b07160cf` on 2026-08-16: Browser projection code still selects
  `display_mode`, preferring `webrtc_remote_display` when the selected engine
  advertises it. This is an open authority-placement gap. The target contract
  has Browser request a display capability while Runtime selects the display
  path and engine adapter.
- Docker/Selkies is only `managed_baseline_not_final_product`; the hosted Selkies/GStreamer service is a managed baseline, not accepted as the final Browser.
- Hosted tooling supports per-launch targets. Each engine/control service keeps
  its declared page capacity; concurrent product sessions require their own
  installed capacity and cleanup proof.
- This server is not a product native-browser proof target because it lacks a real host compositor/display, host audio service, and working network namespace support.
- Verified on the public seed on 2026-08-16: `test -e /dev/kvm` returned 1. The
  seed is a bootstrap and gateway host and may consume a remote Browser Engine;
  it is not a person's Home or a local crosvm/KVM Browser target.
- Kasm Workspaces, BrowserBox, or KasmVNC cannot replace Selkies until the
  operator_control_socket not provisioned blocker and their operator evidence
  requirements are cleared.
- `scripts/browser-provider-decision-report.mjs` summarizes supplied `hosted_bakeoff` and `native_preflight` artifacts and keeps generated placeholder configs out of operator instructions.
- `scripts/browser-provider-runbook.mjs` is read-only guidance. Its operator guidance is generated from the actual evidence and should not be treated as a deployment action.
- Current Browser runbooks must keep the stop condition visible: do not keep
  tuning the running Selkies baseline as product architecture.

## Mac VM Proof Boundaries

- The Mac VM acceptance chain recomputes the receipt SHA-256 from the receipt path and rejects auth setup receipts generated after the machine proof.
- The virtual-auth Browser setup path must drive the virtual-auth Browser open viewport by default.
- Profile reset proof must preserve `removed_profile_disk=true`.
- The virtual-auth credential store remains an owner-only local file.
- The handoff exits non-zero until the headed auth setup receipt is bound.
- `scripts/mac-source-home-restart.sh` remains the source-home restart/proof
  helper for macOS target evidence.

## GBA Capsule Truth

- `gba-emulator` is a conditional viewer capsule, not an always-on native
  provider. It carries one browser-targeted mGBA JS/WASM engine and loads that
  engine only after Runtime supplies compatible uCity or Library `.gba`
  content.
- ROM and save bytes cross authenticated Runtime viewer routes. Save state is
  scoped to the launch principal; the engine has no Runtime WASI adapter,
  preopens, environment, socket, FIFO, or direct-network authority.
- `scripts/normalize-gba-engine-imports.mjs` deterministically converts the
  exact pinned upstream Emscripten import label into the capsule-local
  `capsule.local.memfs.v1` boundary. The product artifact imports only that
  local module and its bundled Emscripten environment.
- `scripts/gba-demo-smoke.sh` proves manifest, Runtime route, authorization,
  storage, input-map, and artifact invariants.
  `scripts/gba-opaque-frame-browser-smoke.sh` proves the same capsule assets in
  disposable Chromium: opaque `Origin: null` topology, parent DOM denial,
  changing nonzero framebuffer writes, trusted keyboard input, nonzero emulator
  audio output, on-screen controls, save/reload persistence, and process cleanup.
- The GBA source catalog contains exactly `gba-nonogram` and `gba-ucity`.
  Shared visual tokens remain inside the GBA capsule. GBA stays outside the
  default profile and belongs only to explicit `demo` or `full` profiles.
  Installation, target media, input, save/reload, and cleanup proof remain
  separate gates.

## Model And Assistant Truth

- `model-provider` is the single active model execution path. Runtime registers
  it as a verified native provider and authorizes only the typed
  `offers_list`, `runs_create`, `runs_get`, `runs_events`, and `runs_cancel`
  operations.
- The operator-owned model-provider config lives under the Runtime data root at
  `providers/model-provider/config.json`. Runtime validates the fixed path and
  file security, passes only the raw top-level offers value through Init, and
  keeps backend URLs, credentials, and adapter details below the provider
  boundary.
- A missing installed components manifest or model-provider entry leaves the
  provider unconfigured and unavailable. Runtime does not select a fallback.
- At this earlier checkpoint, the preserved human-test installations reported
  zero offers. The combined source owned the bounded local llama lifecycle,
  verified model admission,
  restart reconciliation and provider-internal Chat Completions/Responses adapters.
  Model-provider source verification passes 204 unit and five process tests;
  paid hosted calls and installed Qwen acceptance retain their separate gates.
- On Linux x86_64/aarch64, the candidate Runtime starts model-provider with a
  kernel filter that denies new Internet sockets to the provider and descendants.
  Runtime owns exact local llama routes through a Unix broker. An isolated Linux
  Home registered the filtered installed provider, and a separate installed
  bridge test completed SmolLM2 under the same filter. The process and artifact
  receipt is `.audit/linux-sec1-confinement/installed-observation.json`.
  Linux Home Assistant use under this filter and a stricter preopened-channel
  engine design still need proof. Public hosted HTTPS remains paused.
- HTTP-job create writes a private pending marker before dispatch. A lost create
  response leaves that request at `settlement_unknown`; same-ID retry and a
  Runtime/provider restart send no second create. A known job ID can be replayed
  only for its original offer, run, request and backend identity. The current
  HTTP-job test route is a local fixture with no qualified upstream request-ID
  lookup or create-idempotency guarantee. The accepted installed evidence and
  upstream qualification are in `.audit/sec1-unknown-create-contract/receipt.json`.
- The current source does not integrate the Codex SDK. Codex remains a later
  agent-execution adapter behind typed agent operations and explicit
  filesystem, network, tool, and approval grants. It is not a model offer.
- `model-provider` now accepts the Runtime Init envelope fields
  `base_path`, `allowed_paths`, `read_only`, `encryption_key`, and `extra`
  without weakening strict unknown-field handling. The zero-offer stdio Init
  test passes with the Runtime envelope in source tests.
- Assistant is the canonical first-party capsule for Sash’s Agent Space,
  conversation UI and composer, with Chat, Build and Studio controls. Home GUI
  owns shelf motion and the frame; Runtime owns protected workspace v2 and model
  authority. Legacy Home Agent launches resolve to Assistant before token creation.
- Source installers retire the old Home Agent capsule tree while retaining its
  protected workspace. Migration preserves all three old stores, full records,
  editable drafts and exact run identities. Concurrent edits retain both versions.
- Typed model controls retain exact CID/offer selection. Studio saves request
  identity before create and keeps per-session drafts, run cursors and output
  history. Historical foreign runs and copied sessions retain their original
  identities without acquiring authority. Copy uses trusted Home Clipboard.
- The source and rendered preservation gates pass. Installed canonical Assistant
  `fa297cb5` passes one catalog identity, legacy launch, a real Qwen reply, exact
  saved/reloaded text and model selection, and owned shutdown/restart. Current
  build/installed/served receipts match. The new human Home has an enrolled passkey account and a locally cached
  Qwen package. System/Marketplace metadata repair `822c4e3d` passes actual
  launch. Anders now confirms successful Assistant use; current `3c2f9a80` also passes the bounded Models/activity follow-up described above. Full human J3 acceptance remains pending. Earlier Home Agent and standalone Assistant observations above are
  historical target receipts. Advanced tools, Library reads, search and broader Studio capabilities
  retain their typed-contract gates.


## System Truth

- System has no generic Storage or pseudo-WebSpace inventory section. Files,
  documents, and provider-backed storage remain in their owning apps; System
  keeps real account, appearance, shell, security, source, app/service, and
  device controls.
- Apps and background services come from the Runtime capsule catalog. Privileged
  identity, permission, verification, and approval details remain behind the
  explicit technical inspection surface rather than ordinary app discovery.

## Home Shell Truth

- `/home/` is the Home front door; `/apps/home/` remains a compatible old entry. The current internal shell model is
  `home-shell-host` for host lifecycle, `home-gui` for the desktop projection,
  and `home-cli` for the command projection.
- `home` remains the installed host/front-door bridge id for `/apps/home/`;
  it is not selectable. `home-gui` and `home-cli` are sibling shell capsules on
  capsule-specific browser origins. They share the same Runtime facts, launch
  validation, lifecycle, sign-out, and explicit shell-switch authority; GUI
  owns windows while CLI owns the Runtime PTY. Visible product language remains
  `Home`.
- `home-cli` replaces the obsolete `esp-shell` capsule as the selectable
  terminal shell. It is a shell-role capsule with no provider authority.
- `home-cli` is a terminal shell over Runtime Home summary, capsule catalog,
  interface, ESP, service, approval, Browser, wallet, people, gate, and audit
  facts. Home CLI TUI actions write a structured Home `intent.json`; the Home
  owner process handles visible app opens and declared runtime-policy affordance
  invocation through Runtime/Home authority. User-approval and high-risk methods
  stay blocked; `home-cli` does not directly call providers or System routes.
- Core first-party manifests now declare typed affordance descriptors across
  app, viewer, shell, connector, content, and provider surfaces. Home-facing
  descriptors cover `home` host facts, `home-gui`, `home-cli`, `browser`,
  `wallet`, wallet connectors, `inbox`, `services`, `system`, `library`, `documents`,
  `archive-manager`, `chat-room`, `assistant`,
  `marketplace`, `gba-emulator`, `gba-ucity`, and `gba-nonogram`; provider-role capsules now
  project authority metadata for service-plane inspection. These descriptors
  are projected as facts for shells and System; Runtime gates, approval, launch
  tokens, providers, and audit remain authoritative.
- `/api/capsules/catalog` now derives `elastos.capsule.projection/v1` for each
  capsule: web, CLI, facts, affordances, gates, audit/mirror, and
  Carrier/service readiness. `home-cli inspect <capsule>` renders these
  Runtime-derived facts instead of guessing from shell-local UI code.
- Machine proof now includes
  `first_party_capsules_have_complete_projection_contract`, which checks the
  first-party development capsule set through the Runtime catalog read model,
  verifies every capsule has the seven shell-facing projection surfaces, and
  confirms `/api/capsules/interfaces` stays count-aligned with catalog facts.
- The browser `home-cli` is now terminal-only in the product path: it autostarts
  a capsule-local xterm.js Runtime PTY stream, focuses the terminal, and keeps
  browser-side command projection out of the product entirely.
- The browser `home-cli` now has a Runtime-owned PTY terminal contract:
  start/events/input/resize/close routes are launch-token gated, event delivery
  uses a scoped stream ticket instead of a Home token, and the capsule renders
  PTY bytes with xterm.js while Runtime owns the process, PTY, dimensions,
  input, resize, and lifecycle. The Home CLI TUI accepts keyboard navigation plus
  SGR mouse wheel movement and tab-row clicks through that PTY.
- Home CLI source is split by existing responsibility into Runtime I/O, line
  views, TUI state, rendering, and view models while preserving one Rust module
  and one snapshot/intent contract. The split is behavior-neutral and does not
  add a framework or command registry.
- `elastos home` / `home-cli` now consumes the shared
  `capsules/home-cli/browser/commands.json` command contract. Home CLI line mode
  embeds it, but first-run help is split into Tabs, Controls, Advanced, and
  Debug. The default path shows only the five user-facing tab commands
  (`home`, `inbox`, `people`, `apps`, `system`) plus controls; `mywebsite`,
  `wallet`, `exits`, `invoke`, `debug`, projection details, raw PTY/xterm
  details, and security wording are hidden until `help advanced`, `help debug`,
  or command-specific help. `home-cli` reads Runtime-derived
  catalog/interface/service facts from the Runtime-owned Home snapshot.
  Low-risk `invoke` still writes a structured Home intent; the Home owner
  process mints a non-delegatable launch token and dispatches through
  `/api/capsules/interfaces/invoke`. Machine proof covers Browser Exit
  service-offer filtering and the serialized Home CLI invoke intent payload;
  user/high-risk methods still fail closed before dispatch.
- Home CLI People now matches the Home GUI People model: profile, contacts,
  pending requests, discovery, and add/remove/message commands stay in the
  default view; room policy, guest sessions, schema/model/source fields, invite
  internals, and transport facts move to `debug people`. Contact message actions
  are available only when Runtime People facts expose a message-capable Home app
  route such as `/apps/chat-room/`; visible contact names and handles resolve to
  the same contact ids as the GUI model, and route-backed actions can say
  `Chat with <person>` without claiming a separate direct-thread transport.
- Home CLI System now keeps the default view to shell switch readiness,
  human session status, trusted source/update policy, and a compact details
  pointer. Services, roots, peers, capsule counts, launch-token/auth wording,
  DID-heavy identity, and detailed diagnostics live under `system source`,
  `system identity`, `system diagnostics`, or explicit `debug ...` topics.
  `system shell home-gui` is ready from browser Runtime PTY mode and unavailable
  from native terminal mode, where no browser root shell is mounted to switch.
- Machine proof covers the signed virtual-passkey System picker switch to
  `home-cli`, full-viewport CLI root mount, isolated `home-gui` root launch with
  no desktop GUI markup or code in the neutral host document, remembered
  alternate-shell first-paint suppression before the Runtime summary round trip,
  stale `home` first-summary suppression before Runtime ensure settles the
  selected shell, no-hint neutral resolving through Runtime ensure before
  selecting `home-cli`, neutral host boot masking until the selected shell,
  auth, or recovery surface is visible, retirement of the previous root shell,
  host-owned neutral auth gating with no
  stale desktop behind the passkey prompt, root-shell-owned window session
  restore, child-intent rejection for wrong-origin, wrong-token, host-route, and
  `home-gui` launch attempts, failed signed switchback recovery without
  mounting `home-gui`, return to `home-gui`, and Home CLI Apps showing
  GUI-only Browser/GBA targets read-only without implicit launch.
  Dynamic `capsule-*` actions come only from the canonical catalog's available
  CLI projection. Browser, GBA, Wallet, and other GUI-only capsules remain
  facts or explicit `open-gui:<target>` actions; Home CLI no longer reloads
  manifests to invent a separate launch matrix.
- Source gates now also assert that GUI chrome projection stays behind
  `home-gui`: the host no longer imports GUI chrome/surface modules or runs
  identity, Inbox badge, Wallet toast, or toolbar clock projection while an
  alternate shell owns the root.
- The Home host no longer queries or mutates desktop/taskbar/launcher DOM nodes
  or GUI window registries. It marks shell lifecycle state and routes validated
  launch-token intents; `home-gui` owns GUI node creation, binding, rendering,
  and window state inside its isolated root frame.
- Home Host summary handling does not require GUI DOM or GUI layout state.
  Desktop layout, browser-window session state, glyphs, and GUI surface state
  live in `capsules/home-gui/browser/shell-core.js`. A normal Home CLI action
  stays in CLI; opening a GUI-only target requires an explicit, launch-token-
  gated `switch shell and open` intent.
- The Home shell source gates assert these ownership rules. The
  origin-isolation change requires a fresh
  commit-bound operator pass covering passkey sign-in, System switching to
  `home-cli`, CLI ownership of the full viewport, no desktop first-paint or
  hidden GUI bleed-through, hard reload into the selected shell, and return to
  `home-gui` without a passkey loop before merge readiness can be claimed.
- `scripts/home-shell-objective-audit.mjs` remains the fail-closed completion
  audit. Manual evidence is commit-bound and intentionally not stored in the
  repository; any later Home shell behavior change requires a new or re-reviewed
  report against the exact reviewed commit.
- Home first-run onboarding now honors the existing `settings=security`
  deep-link, focuses the recovery action when verified readiness becomes
  available, and refreshes Home summary state after Recovery Kit export. People
  setup prefills the suggested first Profile name as editable text, preserves
  unfocused edits across refresh, and still requires explicit create or
  confirm.
- Declared content icons stay capsule-owned and serve only manifest-declared
  icon variants. Nested content entrypoints resolve icons from their matching
  serving root, and declared icon requests reject ROM bytes, traversal, and
  symlinked targets.
- Fresh desktop placement seeds only visible targets on first run. Saved hidden
  target positions remain intact after later reloads.
- Current source proof for the onboarding slice is focused and local:
  `recovery_readiness_change_emits_home_summary_event_only`,
  `test_recovery_readiness_and_first_profile_gate_share_one_recovery_rule`,
  `scripts/people-discovery-smoke.mjs`, and
  `scripts/home-shell-regression-smoke.mjs` pass. The Home audit keeps installed
  outcomes separate from those source tests. Empty-machine recovery coverage
  for a first kit that predates the later random Profile key remains open.
  Manual GUI acceptance still requires the exact installed artifact.
- A fresh Recovery Kit export is now truthful about included People identity.
  Source still needs a separate repair for empty-machine recovery when the first
  kit predates the later random Profile key.

## Collaboration Truth

- Collaboration represents a person through a signed Profile DID. The human
  actor, local principal, signed Profile, and Device DID remain separate. The
  Profile authorizes Carrier endpoint DIDs for routing and scoped signer DIDs
  for application authorship. Neither role is a display name, contact key, or
  browser-visible selector.
- Runtime owns identity derivation, protected state, and Carrier/provider
  mediation. Capsules receive bounded read models and opaque selectors only.
- `chat-room` is the sole Chat product in manifests, demos, build, and
  supported release profiles. The old terminal `capsules/chat` and
  `capsules/agent` source trees are retired and cannot return as a raw-peer
  compatibility path.
- Chat and People use Runtime-mediated collaboration services selected by a
  signed network profile. Runtime owns their registration, workers, shutdown,
  and the long-lived Carrier endpoint. The seed and profile signer are
  bootstrap and configuration authority only, never person, contact, or
  message authority.
- Collaboration messages identify the sender Profile and either a Profile or
  conversation recipient. Runtime routing and Carrier transport no longer
  replace those product identities. A generic acceptance receipt proves only
  that the named endpoint durably accepted the exact envelope; it is not a
  person delivery or read receipt. For direct messages, contact revocations,
  and Profile updates, the receiving Runtime derives the source endpoint from
  the authenticated Carrier connection and checks it independently from the
  Profile-authorized application signer. Shared-room gossip still requires its
  signer to be endpoint-authorized because that admission path does not yet
  receive an equivalent authenticated transport-source fact.
- Peer-DID provider routing resolves through the long-lived Runtime Carrier
  endpoint. The resolved endpoint identity must match the requested DID, and a
  route with no verified peer reports the peer as unavailable before any
  provider effect.
- Verified on `b07160cf` on 2026-08-16: a same-endpoint Peer DID stays on the
  Carrier provider plane and enters authenticated admission through the local
  registry without a network dial. Direct messages settle the same signed
  envelope and signed acceptance receipt used by the remote contract. This was
  implemented by `8dd54706`; it was not an open gap in that snapshot.
- Discovery is explicit, opt-in, bounded, and temporary. Accepted contacts are
  derived from signed request and decision chains, and Inbox is the only
  Accept/Decline authority surface.
- Direct conversations are permitted only between accepted Profile contacts.
  The conversation ID is derived from the two Profile DIDs and the network ID
  and is a selector, not authority.
- Direct delivery is durable on the sender and point-to-point with no relay.
  `send_text_with_context` persists the signed envelope before the first
  delivery attempt, and `retry_pending` re-delivers on the 15-second sync
  cadence until the envelope's 24-hour TTL expires, surviving sender restarts
  (`durable_pending_restarts_with_the_exact_envelope_and_settles_once`). The
  recipient does not need to be online at send time; it needs to become
  reachable within the TTL while the sender's Runtime is running. The real gap
  is a sender that goes offline before the recipient returns: there is no
  third-party store-and-forward, and an expired envelope is abandoned and
  reads `expired`, never `pending`. The seed never sees message plaintext.
  The shared room has the matching reach limit: gossip topic buffers are
  in-memory on whichever peer holds them, so a peer offline past that
  buffer's retention, or across a restart of the holding peer, misses that
  interval; whatever arrives is ingested durably. Profile update catch-up is
  bounded by the 8-revision announcement ring. A contact further behind
  fails closed with an explicit refusal and needs a fresh approval.
- A Profile authorizes exactly one device today. The signed document supports
  several, but the product path always writes the current local device, so
  Profile update delivery covers renaming and carrying a data root to another
  machine rather than pairing a second concurrent device.
- Recovery restores the collaboration identity. The Full Recovery Bundle
  carries the Profile signing seed, its retained revision ring, and the signed
  contact store; a fresh-machine import keeps the Profile DID, authorizes the
  new device through the normal signed-revision path, and accepted contacts
  learn the rebound endpoint from one announcement. An import whose identity
  restore fails is reported incomplete and never claims a complete account
  recovery.
- Historical verification at `b07160cf` records a disposable, fixture-owned
  two-Runtime collaboration journey. Current localhost and public-seed product
  claims require fresh artifact and target evidence for the exact integrated
  commit.
- Bilateral signed contact removal is implemented with the complete People
  states: a pair-scoped signed revocation delivered over the direct channel
  with durable retry, visible removed state on both sides, retained heads as
  the signed name source, and history readable under the declared policy.
- Shared-room attribution is implemented: configured-room rows are named from
  signed Profile truth (own Profile authority, accepted and retained heads,
  membership profile cards) or rendered explicitly unverified; presence is
  liveness-only and durable history is untouched.
- Signed Profile update delivery is implemented: renames travel as an exact
  bounded signed chain over the dedicated `collaboration-profile` provider,
  apply under strict next-revision and chain-hash rules, and re-announce
  idempotently after restart. The two-runtime proof surfaced and fixed a real
  wire gap: the Carrier peer provider plane had never admitted the profile
  provider. Design and boundaries live in
  [docs/COLLABORATION_HANDOFF.md](docs/COLLABORATION_HANDOFF.md); the
  strict fixture-owned two-Runtime source proof covers Recovery,
  distinct Runtime/Profile evidence, opt-in Discovery, exact Inbox approval,
  direct messages both ways, rename, bilateral removal, re-add, shared-room
  continuity, restart continuity, Clipboard, narrow UI, and final People/Chat
  identity scans all passed on two fresh loopback Homes.

## Remote Carrier Exit Evidence

- Operator evidence must reject local redacted artifact hash mismatches.
- Operator evidence must reject local redacted artifacts that still contain private route material.
- Operator evidence must reject stale or route-mismatched hash-bound route-readiness reports.
- Operator evidence must reject stale local installed artifact readiness reports.
- Operator evidence must reject missing route principals.
- Operator evidence must reject local Browser machine-proof artifacts that do not cite the reviewed route target or target host.
- Operator evidence must reject weak evidence that does not cite the reviewed source/exit runtime DIDs and endpoints.
- Operator evidence must reject weak evidence that does not cite the reviewed principal/grant/target/Carrier stream/cleanup route nouns.
- Remote Carrier Exit readiness must remain hash-bound remote route readiness.
- Public-live update planning must stage candidate binaries in a server-side candidate directory before explicit install approval.

## Public Install Truth

- Public-install branch-binary smokes must pin the installer-selected components manifest.
- Public-install branch-binary smokes prevent source checkout `components.json` from leaking into installed-path proof.
- Public-install branch-binary smokes fail if the selected gateway lacks the current `home` setup profile.
- Branch-override public smokes require a staged or published release-compatible
  manifest with the current `home` profile and checksummed artifacts.
- Source/local Carrier setup proof stays in `scripts/local-carrier-setup-smoke.sh`.
- Public install proof for an integrated candidate requires a staged or
  published release-compatible manifest with the current `home` profile and
  checksummed artifacts.
- Set `ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1` only when the publisher relay
  path itself is under review.
- Integrated installed-path proof waits for one reviewed source tree and
  matching stable installation receipts on each target role.

## Open Blockers

- Product Browser completion is not claimed.
- Manual installed-device checks on Mac and Linux/aarch64 targets are still
  required before release handoff.
