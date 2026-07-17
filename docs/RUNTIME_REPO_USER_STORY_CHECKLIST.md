# Runtime Repo User Story Checklist

Release-facing checklist for the new runtime repository.

Use this in two ways:
- as the automatic proof map: which repo command or smoke script proves each story
- as the manual operator guide: what to run on the seed node and installed target hosts

Rules:
- installed target hosts only count on the installed path: `install.sh` or `elastos update`
- seed-node source proofs do not close installed-host acceptance by themselves
- `just verify` is the dev/source gate, should stay hermetic to the worktree under test, and `just verify-release` is the canonical-publisher release-trust gate
- if a Home surface is shipped, it must be installed, launchable, and useful
- if a story is not proven, hide or demote the surface instead of overclaiming
- if a dedicated smoke script no longer exists, say so explicitly instead of pointing reviewers at dead commands
- installed update and portability proof lives in the current public-install helpers, rerunning those helpers against a published gateway via `ELASTOS_PUBLISHER_GATEWAY=<url>`, `scripts/audit-linux-runtime-portability.sh`, and `just verify-release`

## Host Roles

- Seed node:
  - repo checkout
  - local build/test/proof host
  - trusted-source/operator runtime host
- Installed x86_64 host:
  - installed target-machine proof
- Installed arm64 host:
  - installed target-machine proof

## 0.5.0 Handoff Order

Use this order for the reconciliation branch so source proof, public install
proof, and target-device proof do not get mixed together:

```bash
# 1. Current branch, source/review proof
git diff --check
node scripts/home-entropy-check.mjs
node scripts/browser-entropy-check.mjs
bash scripts/check-wci-alignment.sh

# 2. Current branch, installed-style command surface
just candidate-command-audit

# 3. Current 0.5.0 baseline through the canonical public installer/source path.
# Requires a staged or published 0.5.0-compatible manifest with the current
# home profile and checksummed artifacts.
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> \
ELASTOS_BIN_OVERRIDE="$PWD/elastos/target/release/elastos" \
  bash scripts/public-install-identity-smoke.sh
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> \
ELASTOS_BIN_OVERRIDE="$PWD/elastos/target/release/elastos" \
  bash scripts/public-install-home-frontdoor-smoke.sh

# Source/local Carrier setup proof before a candidate gateway exists
scripts/local-carrier-setup-smoke.sh

# Final public install path after publishing 0.5.0
bash scripts/public-install-identity-smoke.sh
bash scripts/public-install-home-frontdoor-smoke.sh

# Optional stricter publisher relay check
ELASTOS_PUBLIC_INSTALL_FORCE_RELAY_ONLY=1 bash scripts/public-install-home-frontdoor-smoke.sh

# 4. Candidate gateway after staging a 0.5.0 artifact set
ELASTOS_PUBLISHER_GATEWAY=<candidate-url> bash scripts/public-install-home-frontdoor-smoke.sh

# 5. Target closeout while a Home-authorized Browser page is open
scripts/jetson-browser-runtime-audit.mjs \
  --host <target-host> \
  --user <target-user> \
  --data-dir <target-elastos-data-dir> \
  --source-dir <target-source-checkout> \
  --require-parity \
  --min-active-crosvm-seconds 3600
```

Then do the manual installed-device pass on each target host: `elastos setup`,
`elastos`, System, Documents, Library, Inbox, People, Services, at least one
app launch/close, and return Home cleanly.
Do not count source-home or seed-node proof as installed-host acceptance.

Browser product readiness is a separate acceptance gate. If it is in scope,
`scripts/browser-objective-audit.mjs` must pass with accepted hosted/native
media proof plus hash-bound manual UX evidence. If it remains red, release
notes must describe Browser as reconciled architecture/proof path, not a
complete product Browser.

## Release-Critical Stories

| ID | Story | Automatic proof | Seed node manual | Installed x86_64 manual | Installed arm64 manual |
|---|---|---|---|---|---|
| RS-00 | Repo gates are green | `just verify` | Inspect failures, keep worktree clean enough to trust gates | n/a | n/a |
| RS-01 | Trusted install/update path works | `scripts/public-install-operator-smoke.sh`; after publish, rerun it with `ELASTOS_PUBLISHER_GATEWAY=<url>` | Verify source host serves the expected signer/trusted source | `install.sh` or `elastos update`, then `elastos source show` and `elastos update --check` | same as installed x86_64 |
| RS-02 | DID-backed identity works | `scripts/public-install-identity-smoke.sh` and `scripts/local-identity-profile-smoke.sh` | `elastos identity show`, `nickname set/get`, Home identity surfaces | same on installed path | same on installed path |
| RS-03 | Home front door works | `scripts/local-carrier-setup-smoke.sh`, `scripts/home-frontdoor-smoke.sh`, `scripts/public-install-home-frontdoor-smoke.sh` | launch `elastos`, open System/Documents/Library/Inbox, return Home | `elastos` -> Home -> System/Documents/Library/Inbox -> Home | same as installed x86_64 |
| RS-04 | Native chat works | `scripts/local-carrier-chat-smoke.sh` where applicable | open Chat locally, verify send/receive and `/home` / `/quit` | `elastos` -> Chat, exchange messages with another installed host | same as installed x86_64 |
| RS-05 | Shared Chat runtime works | `scripts/shared-runtime-gossip-proof.sh` | native Chat and Chat Room share one runtime-backed conversation state | same as seed node on installed path | same as installed x86_64 |
| RS-06 | Full-screen chat microVM works | `scripts/chat-demo-local-smoke.sh` on KVM hosts; installed-path proof is manual on this line | source-local KVM proof if applicable | `elastos setup --profile chat`, then direct packaged chat | same as installed x86_64 |
| RS-07 | MyWebSite is useful | covered partly by Home frontdoor smokes and site command tests | staged preview opens, `Go public`/ephemeral exposure gives a URL when installed, and any surfaced Home action is truthful | same as seed node on installed path | same as installed x86_64 |
| RS-08 | Documents and Library are useful | `scripts/home-camofox-smoke.sh`, `cargo test -p elastos-server --lib documents -- --nocapture` | create/save/publish a document, then open it from Library | same as seed node on installed Home | same as installed x86_64 |
| RS-09 | Runtime-backed GBA works | `scripts/gba-demo-smoke.sh`; `scripts/gba-linux-browser-smoke.sh` in Linux Chromium | verify uCity and Library `.gba` launch, video/input/audio, save persistence, and cleanup | disposable Linux Chromium proof plus installed conditional demo profile | same capsule artifact as installed x86_64 |
| RS-10 | Updates surface is honest | `scripts/public-install-operator-smoke.sh`; after publish, rerun it with `ELASTOS_PUBLISHER_GATEWAY=<url>` | `elastos update --check`, verify source/runtime state | CLI update status is truthful; compare any surfaced Home/System update action only if visible | same as installed x86_64 |
| RS-11 | Sovereign room sync works | exact local cross-runtime room gateway tests | seed room, pair both runtimes, verify join/leave before and after chat, then exchange a room message and one attachment | same with one other installed runtime | same as installed x86_64 |
| RS-12 | Operator remote control works | `scripts/public-install-operator-smoke.sh` and exact local operator two-node test | allow the controller DID on the target, then run remote `node status`, `node room`, and `node update --check` | act as controller or target | same as installed x86_64 |
| RS-13 | Protected-content provider boundary is testable | `scripts/protected-content-provider-contract-smoke.sh` | verify DRM/rights/key/decrypt providers expose blocked authority, reject invalid requests, and fail closed until backends exist | Library protected-content rail is visible only as disabled/read-only readiness/status; encrypted-recipient sharing stays disabled | same as installed x86_64 |

## Story Details

### RS-00 Repo gates are green

Automatic:
```bash
cd <repo-root>
just verify
```

Pass when:
- `alignment-check`
- clean-home setup plus Home front door smokes
- command smoke
- fmt
- clippy
- tests

all pass in one run.

### RS-01 Trusted install/update path works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-operator-smoke.sh

# after publishing a candidate gateway
ELASTOS_PUBLISHER_GATEWAY=<published-url> bash scripts/public-install-operator-smoke.sh
```

Manual on installed hosts:
```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos --version
elastos source show
elastos update --check
```

Pass when:
- install succeeds
- trusted source is stamped
- node id is present
- operator-side `node status` and `node update --check` are coherent
- the same installed-path operator smoke succeeds against the published gateway when a candidate exists

### RS-02 DID-backed identity works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-identity-smoke.sh
```

Manual on seed and installed hosts:
```bash
elastos identity show
elastos identity nickname set <nick>
elastos identity nickname get
elastos
```

Pass when:
- DID exists
- nickname persists
- Home identity surfaces reflect the same nickname

### RS-03 Home front door works

Automatic:
```bash
cd <repo-root>
bash scripts/local-carrier-setup-smoke.sh
bash scripts/home-frontdoor-smoke.sh
bash scripts/public-install-home-frontdoor-smoke.sh
```

Manual on installed hosts:
1. Run `elastos`
2. Confirm Home renders
3. Open `System`
4. Open `Documents`
5. Create and save a note
6. Open `Library`
7. Open the note from Library
8. Open `Inbox`
9. Return Home after each app

Pass when:
- Home opens cleanly
- child surfaces return home cleanly
- notices are useful, not misleading

### RS-04 Native chat works

Automatic:
- use current local Carrier chat smoke where applicable

Manual on installed hosts:
1. Open `elastos`
2. Enter `Chat`
3. Exchange messages between two installed hosts
4. Verify your own send is echoed locally
5. Exit with `Esc`, `/home`, and `/quit`

Pass when:
- delivery works both ways
- no duplicate delayed replay
- no runtime logs leak into the UI

### RS-05 Shared Chat runtime works

Automatic:
```bash
cd <repo-root>
bash scripts/shared-runtime-gossip-proof.sh
```

Manual on seed node:
1. Launch native chat
2. Launch Chat Room on the same runtime
3. Exchange messages both ways
4. Verify same-host interop uses the shared runtime state

Pass when:
- lower-layer gossip proof passes
- end-to-end native ↔ WASM smoke passes

### RS-06 Full-screen chat microVM works

Automatic:
```bash
cd <repo-root>
bash scripts/chat-demo-local-smoke.sh
```

Installed-host proof for this story is currently manual on this line.

Manual on installed hosts:
```bash
elastos setup --profile chat
elastos capsule chat --lifecycle interactive --interactive --config '{"nick":"<nick>"}'
```

Pass when:
- direct full-screen chat works
- microVM TUI is usable and returns home

### RS-07 MyWebSite is useful

Automatic:
- covered partially by Home frontdoor smokes

Manual on seed and installed hosts:
1. Stage a simple site
2. Open the local preview with `elastos open localhost://MyWebSite`
3. Confirm the local preview URL is useful
4. If the Home UI surfaces MyWebSite actions, confirm they match the CLI state
5. Trigger `Go public` or `elastos site serve --mode ephemeral` when the required components are installed
6. Confirm temporary HTTPS URL works

Pass when:
- preview opens
- public URL path is clear
- any visible Home notice tells the user what to do next

### RS-08 Documents and Library are useful

Automatic:
```bash
cd <repo-root>
bash scripts/home-camofox-smoke.sh
cd elastos && cargo test -p elastos-server --lib documents -- --nocapture
```

Manual on seed and installed hosts:
1. Open `Documents` from Home
2. Create a document
3. Save it
4. Publish it
5. Open `Library`
6. Confirm the document appears as content, not a raw path
7. Open the document from Library
8. Copy the `elastos://<cid>` link and open it from Chat Room or Documents where available

Pass when:
- Documents and Library use the same document identity and provider contract
- published revisions open as immutable `elastos://<cid>` objects
- drafts are clearly local until published

### RS-09 Runtime-backed GBA

Automatic:
```bash
cd <repo-root>
bash scripts/gba-demo-smoke.sh
# On a Linux Chromium target with no product-state mutation:
bash scripts/gba-linux-browser-smoke.sh
```

Manual:
1. Install the `demo` or `full` profile and open a uCity or `.gba` object.
2. Launch the bundled uCity capsule, then open a `.gba` object from Library.
3. Confirm video, keyboard or controller input, and audio work.
4. Reload the same game and confirm its principal-scoped save survives.
5. Close the app and confirm the browser engine workers stop with the capsule view.

Pass when:
- both content-capsule and Library launch paths work
- save persistence actually survives reload
- one portable engine artifact is used on Mac and Linux
- the engine has no Runtime WASI, host filesystem, or direct network authority
- the Linux browser proof exits cleanly and leaves no browser/container state

### RS-10 Updates surface is honest

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-operator-smoke.sh

# after publishing a candidate gateway
ELASTOS_PUBLISHER_GATEWAY=<published-url> bash scripts/public-install-operator-smoke.sh
```

Manual on installed hosts:
1. Run `elastos update --check`
2. If Home or System exposes an update action, open it
3. Compare the message

Pass when:
- CLI and any surfaced UI tell the same story
- no fake `ready/current` message when trusted-source check failed

### RS-11 Sovereign room sync works

Automatic:
```bash
cd <repo-root>/elastos
cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_presence_syncs_join_and_leave -- --exact --nocapture
cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_room_syncs_over_carrier -- --exact --nocapture
cargo test -p elastos-server --lib api::gateway::tests::test_room_service_cross_runtime_attachment_syncs_over_carrier -- --exact --nocapture
```

Manual across two runtimes:
1. On runtime A, run `elastos room seed --title "Review Room"`
2. On runtime A, run `elastos room invite-export <did:key:...> --role member > invite.json`
3. On runtime B, run `elastos room invite-import invite.json`
4. On runtime B, run `elastos room accept <invite-id>`
5. On runtime B, run `elastos room accept-export <invite-id> > acceptance.json`
6. On runtime A, run `elastos room accept-import acceptance.json`
7. Start the explicit operator lane and open the hosted room path on both runtimes
8. Pair both browsers or local room sessions
9. Confirm runtime B sees runtime A join before any text message is sent
10. Confirm runtime A sees runtime B join before any text message is sent
11. Send one text message from runtime A
12. Confirm runtime B receives it
13. Send one reply from runtime B
14. Confirm runtime A receives it
15. Upload one attachment from runtime A
16. Confirm runtime B sees the attachment object and can fetch the bytes
17. Leave the room from runtime B
18. Confirm runtime A sees the leave event and the participant roster shrinks

Pass when:
- owner and guest converge on the same room membership
- both runtimes surface join/leave presence before and after chat
- text delivery works both ways without duplicate replay
- attachment delivery works and fetched bytes match

### RS-12 Operator remote control works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-operator-smoke.sh
cd elastos
cargo test -p elastos-server --lib operator_control::tests::test_two_node_operator_status -- --ignored --exact --nocapture
```

Manual across controller and target runtimes:
1. On the target, run `elastos setup --profile operator`
2. On the target, run `elastos serve`
3. On the target, copy the DID and connect ticket from `elastos node info`
4. On the target, allow the controller DID with `elastos node peer add --did <controller-did> --allow status.read --allow update.check --allow room.read`
5. On the controller, add the target with `elastos node peer add --did <target-did> --ticket <ticket>`
6. On the controller, run `elastos node status --peer <target-did>`
7. On the controller, run `elastos node room show --peer <target-did>`
8. On the controller, run `elastos node update --peer <target-did> --check`

Pass when:
- the target is reachable over Carrier
- `node status` reports the correct runtime kind and version
- `node room show` returns the remote room summary coherently
- `node update --check` reports the trusted source coherently

### RS-13 Protected-content provider boundary is testable

Automatic:
```bash
cd <repo-root>
bash scripts/protected-content-provider-contract-smoke.sh
```

Manual on seed node:
1. Inspect the smoke output
2. Confirm the four provider capsules run through their JSON line protocol
3. Confirm valid requests fail closed until backends are configured
4. Confirm invalid raw-authority requests are rejected
5. Confirm `drm-provider.open` reports the provider/runtime sequence

Pass when:
- DRM, rights, key, and decrypt provider contract checks pass
- raw authority remains blocked from normal capsules
- any visible Library protected-content rail stays disabled/read-only until
  provider/capability proof covers production encrypted-recipient sharing

## Minimum Publish Bar

For the new runtime repo, the minimum honest publish set is:
- RS-00
- RS-01
- RS-02
- RS-03
- RS-04
- RS-10
- RS-11
- RS-12

Everything else must either:
- pass its own story, or
- be demoted/hidden as not yet earned

## Manual Run Sheet

### Seed node

```bash
cd <repo-root>
just verify
bash scripts/shared-runtime-gossip-proof.sh
bash scripts/gba-demo-smoke.sh
bash scripts/protected-content-provider-contract-smoke.sh

# Release-context only: canonical publisher signer required
just verify-release
```

### Installed x86_64 host

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos setup --profile demo
elastos

# if validating room or operator flows on this host
elastos setup --profile operator
elastos serve
```

Manual checks:
- People / identity
- System / Documents / Library / Inbox
- Native chat shortcut
- MyWebSite command path, plus any surfaced Home action if visible
- CLI update status, plus any surfaced Home/System action if visible
- Chat Room after `setup --profile demo` plus `setup --profile operator`
- Full-screen Chat only if you are explicitly closing RS-06 on this host

### Installed arm64 host

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos setup
elastos setup --profile demo
elastos

# if validating room or operator flows on this host
elastos setup --profile operator
elastos serve
```

Manual checks:
- People / identity
- System / Documents / Library / Inbox
- Native chat shortcut
- MyWebSite command path, plus any surfaced Home action if visible
- CLI update status, plus any surfaced Home/System action if visible
- Chat Room after `setup --profile demo` plus `setup --profile operator`
- Full-screen Chat only if you are explicitly closing RS-06 on this host
