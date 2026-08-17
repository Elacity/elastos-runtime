# Tasks

Open work only. Completed work belongs in
[elastos/CHANGELOG.md](elastos/CHANGELOG.md). Verified current truth belongs in
[state.md](state.md).

Operating principle: one canonical path per operation and clear failure when a path is not yet ready.

Guiding-star constraints live in [PRINCIPLES.md](PRINCIPLES.md).

Do not add new product surface area until the `Now` section is materially tighter.

## Now

Read this section as strict priority order for this branch. Do not start a lower
section if a higher section is incoherent, unverified, or too large to review.

### Collaboration candidate closeout

This is the only active closeout for `codex/post-0.6-consolidation`. It is based
on released `main` at `d358dedb` and is a candidate for the next 0.7 development
line. Do not resume work from superseded hardening, network-profile, or
collaboration worktrees.

The source boundary is complete: a person is a signed Profile, a Runtime owns
authority and collaboration state, Carrier transports authenticated endpoint
traffic, and People, Chat, and Inbox use typed Runtime resources. The strict
fixture-owned two-Runtime journey passes. This does not yet prove the installed
product on localhost or the public seed.

The first normal cross-Runtime Chat send exposed a Runtime abort in the old
Iroh 0.96.1 transport generation. The reviewed Iroh 1.0.2 source port is now
part of this closeout. Its source tests pass; localhost now has installed
artifact parity plus machine Browser open/connect/close/zero-residue proof, and
the remaining installed localhost/manual Browser usability plus public-seed
proof remain in steps 3 and 4 below.

Finish these steps in order. Do not add product scope while they are open:

1. [x] Review the complete branch, remove stale or competing truth, and rebuild
   the 47 development commits into a small authority-bound review series with
   the same final behavior.
2. [x] Run the complete source gate on the reconstructed series, including the
   explicit People and Chat capsule gates and the fixture-owned two-Runtime
   acceptance.
3. [ ] Install the exact reviewed commit on localhost and pass one-Runtime
   acceptance: Profile creation and rename, opt-in discovery state, Inbox
   request handling, Chat selection and settings, trusted Clipboard, restart,
   and no unexpected writes from read-only summaries.
   The current candidate is installed with artifact parity and HTTP 200.
   Existing localhost evidence now covers People, Chat, Inbox, trusted
   Clipboard, restart continuity, and a machine Browser open/connect/close path
   with zero ownership/stream/reconciliation residue. Manual Browser visible
   video/input usability on the installed localhost candidate is still open, so
   this step remains open.
4. [ ] Install that same commit on the public seed and pass the real two-Runtime
   journey: overlapping bounded discovery, one request, Inbox-only acceptance,
   direct messages both ways, rename, remove, re-add, shared-room continuity,
   restart, narrow-window UI, and exact source/installed artifact parity.
5. [ ] Review the installed evidence, update release truth, and publish only the
   approved series to the named 0.7 development branch. Do not merge, tag, or
   deploy as a release during this closeout.

Mailbox delivery, user-created groups, silent blocking, wider discovery scale,
the remaining Home redesign, and Browser setup are later work. They must not
enter this candidate to satisfy the five gates above.

## Later

### Collaboration identity and Carrier boundary

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

#### Migrate the stranded UI work

`feat/shell-ui-esp-on-protocol-extended-ai-work` and `feat/shell-ui-v1` hold
substantial UI work that never reached `main`. The newer branch is 377 files and
62,806 insertions, 35 commits behind `main`, and built against pre-0.6
contracts. Merging is not viable. Migration is per surface, rewritten against
current contracts, each piece verified on its own.

The branch mixes four things and they must not travel together:

1. a shared design system replicated into 17 capsules, which has landed as one
   canonical source under `capsules/_shared/` with a stamping step and a drift
   gate, adopted by Home, Home GUI, People, Inbox and Chat Room;
2. a Home shell redesign, 207 files and 33,600 insertions across `home-gui` and
   `home`, which is the largest single piece of value and is what remains;
3. an agent harness, `agent-harness.js` and `agent-harness.css`, roughly 6,300
   lines, which this file already excludes as extended AI work;
4. pre-0.6 server behaviour, including `auth_gateway.rs`,
   `elastos-identity/src/webauthn.rs`, `gateway_inbox.rs`, `gateway_models.rs`,
   and `gateway_provider_proxy.rs`, which would regress released authority work.

Only 1 and 2 migrate. Nothing from 3 or 4 comes across.

Correcting a constraint this section used to carry: it said no migration commit
may touch `elastos/crates`. That held while migration meant copying assets, and
it stopped holding at the app icons. The branch keeps icons in a central table
keyed by capsule name, which is the coupling the shell already had; letting each
capsule own its icon needs a manifest field and a Runtime-resolved read model,
so that slice touched `elastos/crates` by design. The rule that actually matters
is the one above: nothing from 3 or 4, and no pre-0.6 server behaviour. A
Runtime change a migrated surface genuinely requires is a rewrite against
current contracts, which is what this section asks for.

The Home shell redesign migrates in slices, not in one move. Five have landed
— the restyle of the surfaces that already existed, the app menu bar,
Spotlight, Stages with Exposé, and the keyboard layer with Quick Look — and
the rest are listed below. What made slicing possible is that
the redesign is additive: 71 of the current template's 73 classes survive it,
so each surface can arrive with its own module, markup, styling and gate
assertions, leaving `just verify` green in between.

Three constraints shape every remaining slice. The agent harness does not come
across, and it is woven through `shell-stages.js` (an Agent Space in the ring),
`shell-windows.js` (a workspace snapshot hook) and `shell-surface.js`, so each
needs excising rather than copying. `agent-tip.js` is misnamed on the branch —
it is the shared cache-bust constant with an agent checklist attached, and
should arrive as a shell asset-version module without it. And the branch
predates `home-gui-authority.js` and the `home:shell-context` message, so
neither may be dropped on the way past. The reconciliation test prerequisite
is complete: `BrowserCallRecorder` appends each exact provider call before
publishing its committed count through a watch notification, with 13
consecutive exact-regression runs and eight consecutive complete 11-test
module runs passing without a larger yield or time budget.

- [ ] Migrate the status rails: Notification Centre, the Wallet rail, the Inbox
  rail and the connector sheet. These hang off the menubar clock and the wallet
  bar entry, and `shell-notifications.js` depends on both rails, so they travel
  together. The Inbox rail is what the branch Inbox's `inbox:pending-count`
  message talks to.
- [ ] Migrate the Control Centre, without the branch's Nearby section. Settled:
  Discovery belongs to People, so the shell neither writes it nor reads it. The
  branch does both — it posts to a shell-owned `/api/apps/home/discovery`, and
  it reads `summary.people.discovery`. Neither exists here, and the second is
  the one worth being explicit about: discovery state is returned only from
  `/api/apps/people/summary`, behind People's own launch token, and the Home
  summary carries no discovery field at all. So a "read-only projection" is not
  free — it would mean widening the shell's read model across the exact
  authority boundary the Runtime enforces today, to give the shell standing
  read access to a privacy-sensitive fact it has no authority over. The row
  goes. What survives is everything the shell genuinely owns: theme, accent,
  sounds, focus, dock behaviour, desktop icons.
- [ ] Decide how a Home tells you that you are still discoverable. Dropping the
  Nearby row leaves a real gap, and it is a privacy one: discovery is a
  time-bounded broadcast, and someone who turns it on and closes People has no
  ambient signal that it is still running. The cheap aligned answer is for
  People to raise a notification, which reaches the shell through
  `summary.notifications` — a bounded read model the shell already renders and
  already has reason to. That keeps the fact People's to publish rather than
  the shell's to inspect. Size this against the ten-minute expiry first.
- [ ] Consider capsule-rendered panels in system chrome, as a surface kind of
  their own. The idea: rather than the shell reading People's state, the
  Control Centre gives People a rectangle and People renders its own discovery
  control inside it, under its own launch token. The shell would then hold
  placement and no authority at all — which is a stronger position than the
  projection it replaces, and stronger than the precedent it resembles. Note
  the precedent runs the other way: in viewer/content the passive party is the
  data capsule and the viewer holds authority (`/apps/{viewer}/?capsule={name}`),
  whereas here the passive party would be the shell. Two pieces already exist —
  the host mints per-app launch tokens, and app frames are opaque-sandboxed
  with cross-origin resource policy, so an iframe in a popover is no different
  from one in a window. Three do not:
  - a panel surface distinct from a window. Every app frame today is a window:
    it lands in `shellState.windows`, the dock and the menubar. A panel must be
    none of those, with its own mount and retire tied to the popover.
  - a compact projection contract. People's entrypoint is a full page; a panel
    needs a declared small view, which is work in People and a product call.
  - an anti-spoofing rule. A capsule drawing inside system chrome can imitate
    system UI. Panels need visible attribution to their app, and probably only
    a user-pinned capsule gets a slot.
  This is a feature, not a migration step, and it does not change the Discovery
  decision above: either way the shell never reads discovery state. Shortcuts
  that open People cost nothing and land with the Control Centre; the panel is
  the richer version of the same answer if the ambient control turns out to be
  worth a new surface kind.
- [ ] Migrate the Home host redesign: `capsules/home/browser/shell-auth.js` and
  `style.css`. The branch also deletes the clipboard host, protocol and client,
  the wallet connector host, and the browser context module — all released
  behaviour, none of which may go with it.
- [ ] Add the desktop-object write path the redesign expects, or drop it. The
  branch's `shell-core.js` posts to `/api/apps/home/desktop/objects`, which has
  no route here; the summary carries `desktop_objects` for reading only.
- [ ] Restyle People, Chat, and Inbox. Their behaviour slices have landed —
  contact removal with the full People states, shared-room Profile
  attribution, the terminal delivery state, the reachability signal, and the
  People/Chat notifications — so the layouts now wait only on the product
  surface review setting the target. The token work above is the foundation it
  builds on, not a substitute for it.
- [ ] Prove the migrated UI. These capsules are outside `just verify`, so each
  migrated surface needs its named commands and the Chat UI needs its wasm
  regenerated so the source-to-generated parity check stays green. The entropy
  check now guards the token adoption itself: it asserts People, Inbox and Chat
  Room name no colour of their own and load the vendored sheet, and both new
  assertions were proven by reintroducing a literal and watching them fail.

#### First run on a clean Home

Walking a brand-new Home by hand — register a passkey, open apps, try to
create a Profile, open Chat — found these in one sitting. They share a
cause worth naming: the shell was assembled and asserted against, but never
actually used by a person on a clean Home, and several of these are code
whose own comments describe behaviour that was never implemented.

- [ ] Give Chat its unified-sidebar layout. The window chrome for a
  sidebar-owning Chat exists as `window-chrome-unified-sidebar` and is now
  opt-in by that class alone; Chat rode along by data-target while its
  sidebar was unbuilt, which left the traffic lights floating over bare
  content, so it uses standard window chrome until the restyle builds the
  real sidebar with the conversation list. The lone "Shared room" tab no
  longer renders as a full-width button: a selector appears only once a
  direct conversation gives it something to switch between.
- [ ] Make `elastos setup` provision the macOS Browser. Browser runs fine on
  macOS — through Apple's Virtualization.framework, not the Linux
  supervisor/proxy/bridge components — but nothing in the install path puts
  that stack on a new Home, so a fresh install fails with "Browser Engine
  failed to start cleanly. No Browser page or VM was acquired." Everything
  it needs is already in this repo and simply unwired: `elastos-vz` builds
  `browser-vz-engine-supervisor`, `scripts/` holds the
  `browser-vm-engine-supervisor`, `browser-vm-control-service`,
  `browser-vm-remote-vz-launcher`, `browser-vm-local-crosvm-launcher` and
  `browser-vm-prepare-rootfs-pool` services, and
  `scripts/browser-source-home-config.mjs --platform darwin-arm64` writes
  the adapter config. None of it appears in `components.json`, so the
  working Homes on this machine got it by hand — which is why Browser keeps
  breaking on every fresh Home. The VM substrate itself (a 31M `vmlinux` and
  an 8G `browser-vm/rootfs.ext4`) also has no provisioning path:
  `browser-vm-prepare-rootfs-pool` builds a pool but nothing calls it from
  setup. Wire the whole set into the manifest and the setup profile so a new
  Home gets a working Browser, with `browser-vm-engine-preflight.sh` as the
  check that says which piece is missing.
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
- [ ] Close the verification blind spot, or keep naming it per slice. Only two
  capsule crates are `elastos` workspace members, so `just verify` never lints or
  tests the other twenty-two, including `chat-room-ui`, `chat`,
  `chain-provider`, and every provider capsule. The remaining People and Chat
  work that touches these crates is the UI restyle, so a green `just verify`
  will not prove it. The six Chat UI `needless_return` findings are cleared
  and the checked-in wasm regenerated; the structural blind spot itself
  remains.
- [ ] Answer two Home-shell questions left over from the archive review, then
  remove `archive/detached-commit5-redo`. The content review found nothing to
  import, but two assertions from `1c38cf01` have no representation anywhere in
  the tree: an alternate root mount failure showing a host-owned recovery
  surface, and advanced help keeping explicit power-user commands. Both are
  Home-shell concerns outside the collaboration boundary and neither is a
  security invariant. Decide whether the 0.6 shell dropped them deliberately.
- [ ] Prove the old `codex/0.6-release-hardening`,
  `feat/collaboration-network-profile`, and
  `fix/collaboration-chat-session-bootstrap` lines are contained by patch or
  ancestry, then remove their clean worktrees and local branches.
- [ ] Decide how localhost should run again. It is intentionally offline after
  the copied-checkout teardown, and that host has no installed `elastos` binary
  to fall back on, so bringing it back needs an explicit released-artifact
  decision rather than a build from an unpublished branch.
- [ ] Inventory the remaining historical branches, archive refs, worktrees, and
  stale remote-tracking namespaces. Bundle or retain unique evidence; remove only
  clean, proven duplicates. Do not mix pre-0.6 archive work into this post-0.6
  product branch.

### 0.6 release follow-up boundary

- [ ] Keep Browser included but explicitly limited: address intermittent
  restart, non-retained `ela.city` login, and slow performance before claiming
  full Browser reliability. Preserve exact-once Wallet approval and
  Runtime-only networking while fixing these issues.
- [ ] Decide the post-0.6 policy for plaintext principal roots. The current
  hidden upgrade migrates only roots that already have protection metadata; it
  is not a general 0.5-to-0.6 data migration. Either provide an explicit,
  user-approved reset for unprotected roots or design a separately reviewed
  migration, then remove compatibility machinery that has no supported user
  journey.
- [ ] Replace administrative Browser cleanup retirement with provider-owned
  durable child/process identities and idempotent terminal cleanup receipts.
  Restart recovery must settle exact obligations from explicit retained
  identity, not process-list, port-availability, or socket-inactivity inference.
- [ ] Review `feat/shell-ui-esp-on-protocol` as a post-0.6 product line. The UI
  migration section in `Now` covers what to lift from it and what to exclude. Do not
  replay its obsolete protocol/auth history or extended AI work; require small,
  independently testable UI commits against the released ESP contracts.
- [ ] Resume Carrier reconciliation for 0.7 only after its provider generation,
  multi-node physical evidence, cleanup, and release boundaries are reviewed.

### 0. Branch readiness and reviewability

Branch assumptions: `main` is the released 0.6.0 line at `d358dedb`;
`codex/post-0.6-consolidation` is the sole active local post-release
integration line and remains unpublished until this checklist's verification
and review items pass.

- [ ] Keep this branch reviewable: split changes into coherent commit slices with no corrective commits, no hidden migrations, and no unrelated local artifacts.
- [ ] Keep oversized-file cleanup frozen unless branch review exposes a concrete no-behavior blocker. The existing Browser/Wallet/provider cleanup is already split into focused sibling modules: Browser gateway, Wallet gateway, Wallet UI send/receive/create/request/state/preference flows, wallet-provider EVM crypto, and wallet-provider approval test groups. Keep those seams stable and verified. Do not split `capsules/browser/browser/browser.js` further unless a diagnostic-frame/session seam is proven mechanical and behavior-free. Treat `gateway_tests/room.rs`, `gateway_room.rs`, `gateway_tests/home_system.rs`, `room_service.rs`, `auth_gateway.rs`, and `home_cmd.rs` as later cleanup unless they become direct release-review blockers. Keep `scripts/home-entropy-check.mjs` as a broad alignment gate for now, but do not let it accumulate new product logic. Each future split must be no-behavior, separately testable, and covered by the narrow Rust/JS smoke commands for that surface.
- [ ] Review this branch in authority-bound slices, not as one Browser mega-diff: content availability/protected content providers, chain provider core, auth/recovery core, Wallet authority surface, Home/System UX, Chat/Carrier updates, capsule authority manifests, Browser ABI/adapter, Browser proof tooling, shared runtime/gateway, then release/registry/docs. Each slice must be a coherent commit with its own verification commands. Shared runtime/gateway hunks require manual hunk-level review because they cross provider boundaries. Keep `chain_provider_core` separate from Browser: typed proof, prepare, broadcast, sync health, and node lifecycle are blockchain-quadrant provider work, even when Browser consumes them through Wallet. Keep `auth_recovery_core` separate from route wiring: passkey/WebAuthn verification, proof-bound sessions, principal roots, and Recovery Kit helpers are authority primitives consumed by Home/System/Wallet gateway routes. The Wallet authority surface should be reviewed as provider authority core, Wallet app and connector capsules, then gateway/Inbox/audit wiring only after shared gateway hunks are isolated. For Home/System UX, run `node scripts/home-passkey-virtual-auth-smoke.mjs` on loopback Home to prove signed passkey journeys without a human cookie, then run the Camofox smokes for layout coverage. For Browser ABI/provider work, run the Browser Rust tests, `scripts/check-wci-alignment.sh`, `node scripts/home-entropy-check.mjs`, `node scripts/browser-display-mode-smoke.mjs`, `scripts/browser-wallet-bridge-smoke.sh`, and `scripts/browser-glide-wallet-smoke.sh`. Browser proof tooling must keep provider decision reports, objective audits, and runbooks structured and fail closed while product media/manual evidence is missing. Each slice must name its verification commands and must not claim Browser completion unless `scripts/browser-objective-audit.mjs` passes with accepted product media plus matching manual UX evidence.
- [ ] Keep the accepted 0.6 reconciliation closed and reviewable: ESP, Wallet,
  Recovery, Home authority, GBA, and the bounded Browser continuation are
  reconciled in [state.md](state.md). Carrier reconciliation, the shell UI
  redesign, and extended AI UI work remain excluded. Before claiming
  completeness, run `git diff --check`, the Home and Browser entropy checks,
  WCI alignment, `just candidate-command-audit`, and touched-surface tests.
  Reopen a slice only for a newly proven release-candidate defect with a named
  owner and verification command.
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

### 0.6.0 release-candidate preservation
- [ ] Review execution order for preserving the accepted 0.6.0 candidate on
  its `upstream/0.6-dev` base:
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
     `scripts/local-carrier-setup-smoke.sh` before a candidate gateway exists.
     Candidate public install proof with the branch binary needs a staged or
     published 0.6.0-compatible manifest with the current `home` profile and
     checksummed artifacts; then rerun `scripts/public-install-identity-smoke.sh`
     and `scripts/public-install-home-frontdoor-smoke.sh` with
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
- [ ] Release package/registry slice: stamp or verify provider component checksums, keep the current `home` publish preflight/dry-run receipt valid, verify installed provider manifests on the target data dirs, decide whether Browser VM helper/rootfs/initrd artifacts remain source-home generated via `scripts/setup-source-home.sh` plus `scripts/browser-vm-target-refresh.sh` or become explicit `components.json` release components, publish the 0.6.0 binary/artifact set so no-override public installed-path smokes use current code, and make changelog/release claims match only the proofs that passed on real target hosts.
- [ ] Final entropy slice: remove only proven-unused reconciliation leftovers, stale display paths, stale work logs, duplicate truth surfaces, generated artifacts, and target backups after they are either archived or intentionally retained. Do not add compatibility shims for removed `runtime_frame`, `diagnostic_frame`, screenshot, image-polling, or host-specific browser paths unless a current shipped caller is proven.

### 1. Blockchain quadrant: identity, wallet, auth, node capsules
- [ ] Enforce the blockchain quadrant contract in code before UI: runtime principal, verified proof bindings, short-lived session grants, scoped capabilities, provider-mediated effects, signed audit, and fail-closed behavior.
- [ ] Keep `scripts/wallet-product-safety-smoke.sh` green before release publish. It is the product-level Wallet safety gate for MetaMask multi-account link/remove, passkey-gated built-in account delete and recovery-key export/import, WalletConnect disabled without pinned operator config, Ledger hidden until implemented, and no hosted Browser UniSat injection path.
- [ ] Make recovery semantics impossible to misunderstand before release publish: System's `Download Recovery Kit` must export one password-protectable full bundle containing the principal-owned Home/user data root plus every recoverable built-in Wallet key for that principal. Individual `elastos.wallet.recovery-key/v1` export/import remains an advanced per-account escape hatch. External wallets such as MetaMask, WalletConnect, Ledger, Essentials, and UniSat can only restore links/metadata because their private keys live outside ElastOS. Deleting a built-in wallet must warn when no full bundle or individual Wallet key has been saved, and the main Wallet view must offer both `Create account` and `Import Wallet key` without sending users to Settings.
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
- [ ] Replace the current `route + attach_kind` launch payload with a runtime-issued launch grant that is transport-agnostic and non-delegatable.
- [ ] Add an explicit runtime/manifest exposure contract for Home, gateway, and shared surfaces so internal-only and external-only objects do not depend on name-based filtering.

### 3. Home front-door boringness
- [ ] Prove one boring installed `elastos -> Home -> app -> Home` path on Jetson and WSL.
- [ ] Keep tightening dashboard navigation, return-home behavior, and single-owner TTY/session rules until target-machine proof is boring.
- [ ] Keep unfinished surfaces out of the main live path unless they launch from Home and return cleanly.
- [ ] Rehearse and simplify the Home/People/Spaces/System story so the front door feels useful without internal-runtime narration.
- [ ] Extend `elastos.runtime.services/v1` beyond local configured-provider cards and conversation offers: remote Exit, storage, relay, model, and hosting offers must arrive as provider-backed `elastos.service.offer/v1` records through People/Carrier, and enabling one must create/select a principal-scoped provider grant instead of giving capsules direct People-state authority.
  - [ ] Model Provider subtask: the current prototype is refactor evidence only,
    not product truth or a compatibility path. Today `capsules/ai-provider`
    routes caller-selected backend strings to local OpenAI-compatible HTTP,
    Venice, or the Codex CLI; `capsules/llama-provider` owns a
    `llama-server` child and loopback endpoint; `server_infra` starts native
    installed provider binaries through `ProviderBridge` and passes the llama
    URL into `ai-provider`; `provider_resource` derives
    `elastos://ai/<backend>/<op>`; `list_backends` exposes `api_url` or command
    paths; there is still no typed stream/cancel contract, remote service
    offer, Carrier provider path, or principal-scoped backend secret; and the
    `ai-provider` and `llama-provider` manifests still say `microvm` while the
    active Runtime path spawns native provider binaries. Replace that with one
    typed Runtime Model service: Runtime selects a granted provider instance,
    provider adapters own URLs, keys, model processes, model discovery,
    invoke/stream/cancel, limits, and usage receipts, local llama, remote
    Spark/Jetson, OpenAI, Claude, and Venice all implement the same contract,
    remote instances publish signed provider-backed model service offers and
    use Carrier only below Runtime routing, and capsules or agents receive no
    backend string, URL, port, API key, endpoint DID, or Carrier peer. Keep a
    full Runtime on Spark/Jetson as the first supported remote host and define
    a smaller provider host only after it has identity, lifecycle, update,
    audit, and recovery parity.
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
- [ ] Keep the visible Browser on the stream/engine path, not host iframe or host tab browsing. Address requests must call `/api/apps/browser/open`; Runtime reserves streams through Net/Exit; Browser UI receives only a page id plus frame/input routes; `elastos://net/http` remains compatibility/diagnostic-only.
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

The published source-only protected-content stack currently ends at
`origin/feat/protected-content-key-reconstruction`: canonical contracts,
custody and node operations, then decrypt-boundary reconstruction. Remaining
work must stay in this order. Carrier remains transport only throughout every
stage below.

The published custody branch defines the source-only EVM
`has_access_by_content_id` policy/evidence contract, including the exact
contract right string mapped from one product action, Profile-signed
recipient-key authorization, signed immutable custody epoch, and authenticated
replay-pending Runtime release-operation envelope. It also adds the node-local
durable dual-key replay store and the private claim-gated transition from
authenticated replay-pending evidence to an exact persisted encrypted
contribution. Exact retries replay only that result after restart. Remaining
tasks start after those boundaries; they do not reopen them as ambient provider
configuration. Runtime owns its own orchestration replay. Each custody node
owns only its local claim and result. A crash after the claim becomes durable
but before the result becomes durable remains fail closed and requires a fresh
Runtime release operation; there is no operation-resume journal for that state.

#### A. Source-only review gate

- [ ] Obtain external cryptographic, custody, and contract review of
  `docs/PROTECTED_CONTENT_CONTRACTS_V1.md`, the source-only
  `elastos-protected-content-contracts` crate, and the source-only
  `elastos-protected-content-custody` crate before any provider or product
  integration. The branch-local source review already closed with no code
  findings after the shared strict DID codec and Carrier codec consolidation;
  that was not an external cryptographic audit or production security
  approval. An independent AI/model review later found and prompted the local
  custody remediation for invalid X25519 contract bytes, exact released
  threshold settlement, and post-reconstruction CEK commitment checking; that
  review is also not a professional external audit. Review the canonical
  codec, domain tags, field bounds, EIP-191 recovery (`0/1` only), canonical
  Profile `did:key`, policy identity and body, recipient-key authorization,
  owner-Wallet recovery, Runtime operation issuer binding, atomic replay keys,
  signed custody epochs, node decisions, terminal issuer signatures, nested
  authority windows including contribution-to-terminal settlement, shared
  canonical Ed25519 validation (including noncanonical-encoding rejection),
  the stricter local canonical X25519 contract-key rule, custody-envelope AAD
  domains, pinned HPKE share sealing, GF256 Shamir share handling,
  exact-threshold release settlement, threshold reconstruction zeroization,
  manifest-bound reconstructed-key commitment checking, unique contribution
  commitments at threshold settlement, and golden vectors.
- [ ] Add professionally reviewed wrapper-level known-answer coverage for the
  pinned HPKE share framing if a grounded upstream vector path becomes
  available. The current source keeps tamper and binding tests, but the pinned
  upstream `hpke` 0.13 vector set does not cover our exact fixed 32-byte share
  wrapper framing.

#### B. Source-only custody node operations and durable node state

- [ ] Complete the remaining custody operational design around the reviewed
  epoch and release-operation contracts: node admission and rotation, review or
  replacement of the pinned threshold and recipient-encryption suites,
  revocation, audit retention, issuer-key lifecycle, and recovery operations.
  The published source-only stack through
  `origin/feat/protected-content-key-reconstruction` now implements new
  content share provisioning, recipient-sealed node release, and
  decrypt-boundary threshold reconstruction, but it does not yet prove full
  operational custody safety, complete node-state durability, or product
  integration. Its manifest-bound CEK commitment detects a wrong
  reconstructed key; it does not identify the malicious node or add verifiable
  secret sharing.
- [ ] Extend the reviewed node-local replay-claim and claim-gated release
  transition into full operational node state: retained claim pruning policy,
  multi-process operational audit retention, node admission and issuer-key
  lifecycle, recovery tooling, and storage review for production hosts. The
  published custody branch already proves one owner-only durable dual-key
  claim store, atomic no-partial-claim updates, restart survival, private
  claim-before-release authority, exact encrypted-result persistence, and exact
  retry replay. It does not yet add an operation-resume journal for a claim
  that became durable before its result, so that state still requires a fresh
  Runtime release operation.

#### C. Atomic Runtime/provider cutover plus source allow/deny proof

- [ ] Add a typed protected-content Wallet operation that verifies an
  externally completed signature, canonicalizes a `27/28` recovery byte to
  `0/1` after signer verification and before `WalletSignedRightsRequestV1`
  construction, and covers managed, MetaMask, and WalletConnect paths with
  exact tests. The v1 contract itself continues to accept only canonical
  `0/1`.
- [ ] Implement how Runtime derives the recipient public key from the
  authenticated Profile and session, proves holder-only key possession, and
  selects an approved encryption suite. The current Profile signature
  authorizes one exact recipient public key and one Runtime operation issuer;
  it does not prove X25519 secret-key possession. The caller must not provide
  endpoint, device, route, or Carrier identity as recipient authority.
- [ ] Implement the Runtime-owned replay claim store and orchestration around
  the reviewed contract values. Runtime must generate a fresh opaque 32-byte
  session binding, fresh rights and release nonces, and bounded time windows;
  capsules must not select replay or session authority.
- [ ] Pin approved chain ids, contract addresses, selectors, and ABI fixtures
  inside provider policy and evaluation evidence for `RightsPolicyBodyV1`.
  The source-only contract already requires those typed inputs; this stage
  chooses the approved production values and wires them into provider policy.
  Keep chain URLs and transport selection outside capsule/provider contracts.
- [ ] Wire Runtime-owned protected-content orchestration only through the
  reviewed v1 contracts: content status/fetch, Wallet-approved rights request,
  recipient-key authorization, application-authenticated release operation,
  independent node rights checks, key release, scoped decrypt/render session,
  terminal receipt, audit, cancellation, and cleanup. Runtime must select
  providers and Carrier endpoints; capsules must not select routes or receive
  contributions or raw keys. Bind the fresh opaque contract value to the
  verified session in Runtime state; never place a session ID, session cookie,
  launch token, capability token, or bearer credential in the contract.
- [ ] Keep the later Runtime open/buy integration aligned with the reviewed
  authority model: viewer launches must use Home projection authority; Runtime
  must derive Wallet v2 authority from verified launch authority; Chain may
  prepare nonce/gas, but approval and broadcast must use the existing durable
  Runtime transaction coordinator. Do not add Creator-local transaction
  workflow, broad chain-scan fallback, debug logging flood, CBC envelope, or
  sidecar/WireGuard/topology path. The public, unmerged PR #15 commits `ffea5998`
  and `e148218b` are source evidence only, not current code or accepted
  behavior.
- [ ] Replace the provisional `elastos_common::protected_content` DTOs and
  their current provider consumers atomically with the reviewed v1 crate during
  the integration slice. Delete the superseded shapes; do not keep parallel
  authorities, compatibility adapters, or fallback decoding.
- [ ] Add reviewed production dDRM rights-method configuration for the approved
  ESC/Elacity contracts only after contract addresses, selectors, and ABI
  fixtures are pinned in tests.
- [ ] Wire `rights-provider` to approved dDRM/chain policy backends through
  typed `chain-provider` reads, with every release node verifying the Wallet's
  exact right independently. Do not put license logic in app UI or
  gateway-only checks.
- [ ] Wire Runtime-selected custody providers to the reviewed threshold-custody
  backend only after
  node contribution authentication, same-node rights evidence,
  recipient-sealing encryption, exact-threshold settlement, durable replay
  rejection, expiry, revocation, and failure cleanup are proven. Verify the
  capsule-visible response is the authenticated terminal receipt, not
  provider-private contribution bytes or key-backend authority.
- [ ] Wire `decrypt-provider` to a real decrypt/render backend that returns
  scoped rendered output or decrypt sessions to the viewer instead of broad raw
  key access.
- [ ] Evaluate decrypt/render/media helper crates as provider-internal
  implementation candidates only after the fail-closed protected-content
  sequence is wired and source-path tests prove capsules receive neither key
  material nor IPFS, chain, Wallet, or route authority. Source contract
  field-name tests are not confidentiality proof.
- [ ] Prove one source allow flow and one source deny flow through the reviewed
  Runtime/provider contract path with no fallback or parallel DTO surface.

#### D. Isolated installed proof

- [ ] Wire real protected-content producers to the existing `sealed` content
  object contract after payload encryption, rights policy, availability
  receipt, provenance, key-envelope, and viewer-interface generation exist.
- [ ] Prove one honest protected-content flow end to end on an installed
  target: resolve object by stable identity, verify trust material, authorize
  access, decrypt for the rightful user, open in the correct viewer/app, and
  fail closed for everyone else.
- [ ] Only after stages A-D, decide whether to add permissioned ElastOS
  PQ-hybrid dKMS custody for new protected content. Do not use FROST as the
  long-term dKMS root; FROST is only a classical helper for receipts or cohort
  decisions.

### Operator and audit hardening
- [ ] Keep the existing SHA-256 audit chain canonical for 0.6 unless an explicit
  versioned migration is approved. BLAKE3 may remain a content/cache/transport
  choice, but an audit migration must add an algorithm id, canonical encoding,
  golden vectors, a signed transition from the retained SHA-256 head, and
  restart/tamper/truncation tests. Remove branch-plan claims that BLAKE3 audit is
  already implemented.
- [ ] Keep `verify`, `command-smoke`, `installed-command-audit`, and related gates honest and fail-closed.
- [ ] Continue the systematic crate audit through the remaining runtime crates.
- [ ] Track the security/platform work intentionally left out of the earlier CVE hygiene pass as explicit follow-up, not hidden release debt: migrate `bincode 1.3.x` to `bincode 2.x` with versioned serialization compatibility tests; coordinate the `iroh`/Hickory fix as a Carrier-generation upgrade with Rust/MSRV/toolchain proof instead of force-overriding transitive DNS crates; review Sash's macOS VZ / `elastos-crosvm` Darwin substrate branch as a separate platform decision; and keep any temporary Hickory audit ignores documented until the Carrier upgrade closes them.

### Dead code cleanup
- [ ] Re-audit `provider/registry.rs` from current source, not from the stale dead-code list that existed before the 2026-03-31 cleanup. Only remove API surface that is now proven unused on the installed path.
- [ ] Continue the crate-by-crate orphaned-code audit with the same fail-closed rule: delete only after proving the installed path does not use it.

## Later

- [ ] Evaluate WebAuthn PRF and passkey-derived wallet keys only after passkeys are stable as runtime-session and wallet-provider approval gates.
- [ ] Define the browser host-adapter model without faking Linux parity, using the Browser/Net/Exit ABI above so server, desktop, mobile, and kiosk hosts expose the same capability contract.
- [ ] Introduce the dedicated browser capsule only after the runtime launch/object contract and Net/Exit provider contract are stable; it should be one dangerous-but-contained app with explicit outbound network capability, not the platform.
- [ ] Decide the longer-term operator packaging path for Codex and related AI/agent surfaces.
- [ ] Add a hosted-key AI provider behind a stable runtime contract.
- [ ] Evaluate public dKMS only after permissioned dKMS has release receipts, share rotation, monitoring, node admission policy, staking/slashing assumptions, and external crypto review.
- [ ] Consider renaming `elastos-server` crate to `elastos-cli`. It is the CLI binary + all commands, not just a server. The current name misleads new developers about what the crate does.
