# Tasks

This is the ordered open release queue. The
[0.7.2 release plan](https://app.notion.com/p/3d6b682adcca81948f78d12abcd677b9)
owns release scope and acceptance. [State](state.md#current-model-foundation-24-september-2026-utc)
owns exact source, installed and public evidence.
[Deferred and future work](docs/DEFERRED_WORK.md) has open work outside this
queue. Dated queues remain available at commit `f0fae26f` and in the linked
audits; they do not assign a current target or owner.

J3/model work remains first. Required work is a useful local Assistant on the
seed and Mac, hosted OpenRouter/Venice through Runtime-owned consent and
provider isolation, then the focused release journeys below. The builder and
monitor remain paused. The next authorized implementation step stays at the
isolated SEC1 installed gate; this scope correction starts no new experiment.
Notion preserves the original MA, AI, CR and other acceptance clauses. Work
marked Later moves through the deferred index rather than this queue.

## Now

Each row requires a receipt with source/tree, dirty patch, built and installed hashes, target identity, operation, result and next proof. Ports are observations, not installation IDs. Preserve human Homes, keys, pins, data and the 10% disk floor. Publication and public mutation require separate exact-candidate approval.

| Order | Open gate and owner | Next proof and stop condition |
| --- | --- | --- |
| 1 | **SEC1 / J3 — hosted consent and model-provider isolation.** Execution owner: Codex for the isolated Mac gate; Linux containment awaits approval. | Build from the committed source and install only on the named isolated Mac Home. Prove artifact parity, saved hosted-offer activation, a held Assistant send with concurrent Inbox End, and zero new dispatch after End completes. Disposition canceled/failed End behavior. Finish generation-aware model activation so a newly admitted model is usable while an old run remains active. Review the Linux containment plan before any seed change. Prove the provider/guard/engine file and socket boundary plus ordinary Inbox Approve/End on matching Mac and Linux artifacts before hosted activation. Include validation/save, exact origin, redirects, DNS/TLS, jobs, cancel/retry and unknown-create reconciliation. Inventory shipped executable-provider manifest claims against actual enforcement before C5. Stop on unmatched artifacts, missing authority or the resource floor. Jev is Optional, off by default, and available only with a compatible OpenRouter/Jev connection; if shipped, prove typed advice while the human decides. |
| 2 | **J3 / CA1 — useful local Assistant.** Execution owner: Unassigned until the SEC1 target closes. | Keep signed SmolLM2 as the small delivery/failure baseline. Verify the signed Qwen3.5-9B Marketplace Get → immediate Use → useful reply → restart reuse on a suitable Mac. First assess Qwen3.5-4B for the seed against the pinned engine, exact weight/quantization provenance, license and authorized catalogue signing. Define a small ordinary prompt set and pass criteria; record complete answers, cold load, output rate, peak memory and 10% disk reserve on named targets. Prove safe Stop, truthful retention, conversation/model/draft preservation and verified warm reuse. Reuse matching earlier receipts; wider Qwen and sharing matrices stay Later. Stop on missing signing authority or resource pressure. |
| 3 | **J1 / J2 — install, sign in and update.** Execution owner: Unassigned; Anders owns human acceptance. | Qualify the selected Linux x86-64 seed and Apple Silicon candidate with visible sign-in progress and recovery. Name signing-key and restart ownership. Prove the stamped 0.7.1 → 0.7.2 hop through the old client, then a second signed update, interruption/recovery and preserved identity, content, grants and drafts. Resolve which public accounts may configure and pay for hosted AI. Linux ARM64 delivery remains Later. |
| 4 | **J6 / CH1–CH8 — Community Chat.** Execution owner: Unassigned. | Choose the delivery route and owner before implementation. Use existing Profile, Chat, Content/Carrier and Inbox contracts. Prove a usable first visit, explicit room authority, signed message exchange, bounded public-room history/catch-up, and membership/draft/unread preservation through restart and update. Include human narrow-screen use. Two directly connected Homes alone do not qualify public delivery behind home routers. Mailboxes, attachments and private groups remain Later. |
| 5 | **J4 — local Mac Browser.** Execution owner: Unassigned. | Name the reviewed source and installed candidate. Prove ordinary navigation, input, audio/video, actual viewer reload/reconnect, Wallet, profile preservation, responsiveness and Close/orphan cleanup. Apply the relevant local-Mac criteria in the Browser contract with matching manual UX evidence. Remote placement and the wider B01–B16 campaign remain Later. |
| 6 | **C5–C7 — freeze, accept and release 0.7.2.** Execution owner: Unassigned; Anders approves exact publication. | Assemble one reviewed candidate for the Required scope in Notion: useful local and approved hosted Assistant on seed/Mac, public Community Chat, local Mac Browser and signed updates that preserve state. Pass combined source, installed, independent and human review. Keep Optional and Later criteria explicit. Merge the separate Node CI correction into the review line and obtain a passing GitHub run. Public cutover, merge, tag and release each require their exact approval. |

## Browser maturity workstream

The [Browser acceptance contract](docs/BROWSER_ACCEPTANCE.md) defines B01–B16. These open boxes retain full qualification criteria. The focused 0.7.2 release requires the local Mac subset; remote placement and the wider campaign remain Later. A partial source or installed result does not check them. J4 in [Now](#now) is the current execution handoff. One Browser/Net/Exit ABI governs local and remote placements. The current Selkies path is a proof baseline, not product audio acceptance. The operator decision report supplies a structured `next_action`; do not spend more branch time tuning Selkies as the product path when a hosted or native target is required.

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

## Open verification controls

The checks below stay open with their owning journey. They are not additional product scope. Use `scripts/public-install-operator-smoke.sh`, `scripts/public-install-identity-smoke.sh`, `scripts/audit-linux-runtime-portability.sh` and the provisional `scripts/protected-content-provider-contract-smoke.sh` at their applicable installed/release boundaries. Keep source/local Carrier setup proof green with `scripts/local-carrier-setup-smoke.sh`; publish the approved version's binary/artifact set so no-override public installed-path smokes use current code.

For J4, retain `scripts/browser-native-supervisor-smoke.sh`, `scripts/browser-native-proxy-engine-smoke.sh`, `scripts/browser-native-supervisor-proxy-smoke.sh`, `scripts/browser-native-operator-config.mjs`, `scripts/browser-native-target-preflight.sh` and `scripts/wallet-connector-transaction-smoke.mjs` for their target proofs. Use artifact-aware `scripts/browser-provider-runbook.mjs --hosted-bakeoff/--native-preflight --manual-ux` and `scripts/browser-provider-runbook-smoke.sh`; `scripts/browser-objective-audit.mjs` remains the completion gate. Freeze new Browser provider implementation until the current blockers are cleared. The deferred provider comparison and current-host stop condition remain in [future work](docs/DEFERRED_WORK.md).

Remote Carrier Exit two-runtime evidence must cite the exact source/exit runtime DIDs and endpoint evidence, an installed artifact readiness report, evidence for route readiness, installed artifact readiness and cleanup, and a local Browser machine-proof artifact must cite the reviewed route target or target host. Compose Inspector, typed Runtime authority, route-readiness, operator evidence, Browser handoff and redacted artifacts are required before a remote-path claim.

## Deferred open work

[Deferred and future work](docs/DEFERRED_WORK.md) preserves open follow-up outside this execution queue. It does not change the Required/Optional/Later labels in Notion. Keep first-party capsule projection validation covered when that work resumes. The production multi-peer availability/storage market and Component Bus v2 remain future work, with their exact open clauses in the linked backlog.
