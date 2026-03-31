# Runtime Repo User Story Checklist

Release-facing checklist for the new runtime repository.

Use this in two ways:
- as the automatic proof map: which repo command or smoke script proves each story
- as the manual operator guide: what to run on the seed node, WSL, and Jetson

Rules:
- WSL and Jetson only count on the installed path: `install.sh` or `elastos update`
- seed-node source proofs do not close WSL/Jetson acceptance by themselves
- `just verify` is the dev/source gate; `just verify-release` is the canonical-publisher release-trust gate
- if a PC2 surface is shipped, it must be installed, launchable, and useful
- if a story is not proven, hide or demote the surface instead of overclaiming

## Host Roles

- Seed node:
  - repo checkout
  - local build/test/proof host
  - trusted-source/operator runtime host
- WSL:
  - installed x86_64 target-machine proof
- Jetson:
  - installed arm64 target-machine proof

## Release-Critical Stories

| ID | Story | Automatic proof | Seed node manual | WSL manual | Jetson manual |
|---|---|---|---|---|---|
| RS-00 | Repo gates are green | `just verify` | Inspect failures, keep worktree clean enough to trust gates | n/a | n/a |
| RS-01 | Trusted install/update path works | `scripts/public-install-update-smoke.sh` | Verify source host serves the expected signer/trusted source | `install.sh` or `elastos update`, then `elastos source show` and `elastos update --check` | same as WSL |
| RS-02 | DID-backed identity works | `scripts/public-install-identity-smoke.sh` and `scripts/local-identity-profile-smoke.sh` | `elastos identity show`, `nickname set/get`, PC2 People | same on installed path | same on installed path |
| RS-03 | PC2 front door works | `scripts/pc2-smoke.sh`, `scripts/pc2-frontdoor-smoke.sh`, `scripts/public-install-pc2-frontdoor-smoke.sh` | launch `elastos`, enter/exit Chat and MyWebSite, return home | `elastos` -> PC2 -> Chat/MyWebSite -> home | same as WSL |
| RS-04 | Native chat works | `scripts/local-carrier-chat-smoke.sh` where applicable | open Chat locally, verify send/receive and `/home` / `/quit` | `elastos` -> Chat, exchange messages with Jetson | same as WSL |
| RS-05 | Chat WASM works | `scripts/chat-wasm-local-smoke.sh`, `scripts/chat-wasm-native-interop-smoke.sh`, `scripts/shared-runtime-gossip-proof.sh` | run `elastos capsule chat-wasm --lifecycle interactive --interactive` or local dev path and exchange with native chat | n/a unless explicitly shipped there | n/a unless explicitly shipped there |
| RS-06 | IRC microVM works | `scripts/irc-demo-local-smoke.sh` on KVM hosts, `scripts/public-install-irc-smoke.sh` as installed gate | source-local KVM proof if applicable | `elastos setup --profile irc`, then direct IRC and `Apps -> IRC` | same as WSL |
| RS-07 | MyWebSite is useful | covered partly by PC2 frontdoor smokes | preview opens, `Go public` gives URL, return to PC2 home | preview from PC2 works, notice is useful | same as WSL |
| RS-08 | Shared is useful | `scripts/pc2-smoke.sh` and `scripts/command-smoke.sh` | `elastos shares list` returns meaningful state | open Shared from PC2 and confirm it is not misleading | same as WSL |
| RS-09 | GBA UCity is useful | `scripts/gba-demo-smoke.sh` | launch from PC2 or direct path, verify viewer, ROM, save/load persistence | if surfaced in installed PC2, verify launch and usefulness | same as WSL |
| RS-10 | Updates surface is honest | `scripts/public-install-update-smoke.sh` plus PC2 smoke coverage | `elastos update --check`, verify source/runtime state | PC2 Updates action returns useful status | same as WSL |

## Story Details

### RS-00 Repo gates are green

Automatic:
```bash
cd <repo-root>
just verify
```

Pass when:
- `alignment-check`
- PC2 smokes
- command smoke
- fmt
- clippy
- tests

all pass in one run.

### RS-01 Trusted install/update path works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-update-smoke.sh
bash scripts/public-linux-runtime-portability-smoke.sh
```

Manual on WSL and Jetson:
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
- update status is coherent

### RS-02 DID-backed identity works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-identity-smoke.sh
```

Manual on seed, WSL, and Jetson:
```bash
elastos identity show
elastos identity nickname set <nick>
elastos identity nickname get
elastos
```

Pass when:
- DID exists
- nickname persists
- PC2 People reflects the same nickname

### RS-03 PC2 front door works

Automatic:
```bash
cd <repo-root>
bash scripts/pc2-smoke.sh
bash scripts/pc2-frontdoor-smoke.sh
bash scripts/public-install-pc2-frontdoor-smoke.sh
```

Manual on WSL and Jetson:
1. Run `elastos`
2. Confirm PC2 home renders
3. Open `Chat`
4. Return with `Esc`, `/home`, and `/quit`
5. Open `MyWebSite`
6. Return home

Pass when:
- PC2 opens cleanly
- child surfaces return home cleanly
- notices are useful, not misleading

### RS-04 Native chat works

Automatic:
- use current local Carrier chat smoke where applicable

Manual on WSL and Jetson:
1. Open `elastos`
2. Enter `Chat`
3. Exchange messages between WSL and Jetson
4. Verify your own send is echoed locally
5. Exit with `Esc`, `/home`, and `/quit`

Pass when:
- delivery works both ways
- no duplicate delayed replay
- no runtime logs leak into the UI

### RS-05 Chat WASM works

Automatic:
```bash
cd <repo-root>
bash scripts/shared-runtime-gossip-proof.sh
bash scripts/chat-wasm-local-smoke.sh
bash scripts/chat-wasm-native-interop-smoke.sh
```

Manual on seed node:
1. Launch native chat
2. Launch WASM chat on the same runtime
3. Exchange messages both ways
4. Verify same-host interop is boring

Pass when:
- lower-layer gossip proof passes
- end-to-end native ↔ WASM smoke passes

### RS-06 IRC microVM works

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-irc-smoke.sh
```

Manual on WSL and Jetson:
```bash
elastos setup --profile irc
elastos capsule chat --lifecycle interactive --interactive --config '{"nick":"<nick>"}'
```

Also verify in PC2:
1. `elastos`
2. `Apps -> IRC`
3. Exchange a message with the other host

Pass when:
- direct IRC works
- `Apps -> IRC` works
- microVM TUI is usable and returns home

### RS-07 MyWebSite is useful

Automatic:
- covered partially by PC2 frontdoor smokes

Manual on seed, WSL, and Jetson:
1. Stage a simple site
2. Open `MyWebSite` from PC2
3. Confirm local preview URL is useful
4. Trigger `Go public`
5. Confirm temporary HTTPS URL works

Pass when:
- preview opens
- public URL path is clear
- PC2 notice tells the user what to do next

### RS-08 Shared is useful

Automatic:
```bash
cd <repo-root>
bash scripts/command-smoke.sh
bash scripts/pc2-smoke.sh
```

Manual on seed, WSL, and Jetson:
1. Create at least one share
2. Open `Shared` from PC2
3. Confirm the screen reflects real channels/next steps

Pass when:
- Shared is not empty theater
- if empty, it explains what to do next

### RS-09 GBA UCity is useful

Automatic:
```bash
cd <repo-root>
bash scripts/gba-demo-smoke.sh
```

Manual on seed, WSL, and Jetson if surfaced:
1. Open `GBA UCity`
2. Confirm viewer loads
3. Confirm ROM boots
4. Save state
5. Reload
6. Load state

Pass when:
- launch works from the surfaced path
- save persistence actually survives reload

### RS-10 Updates surface is honest

Automatic:
```bash
cd <repo-root>
bash scripts/public-install-update-smoke.sh
```

Manual on WSL and Jetson:
1. Run `elastos update --check`
2. Open `Updates` from PC2
3. Compare the message

Pass when:
- PC2 and CLI tell the same story
- no fake `ready/current` message when trusted-source check failed

## Minimum Publish Bar

For the new runtime repo, the minimum honest publish set is:
- RS-00
- RS-01
- RS-02
- RS-03
- RS-04
- RS-10

Everything else must either:
- pass its own story, or
- be demoted/hidden as not yet earned

## Manual Run Sheet

### Seed node

```bash
cd <repo-root>
just verify
bash scripts/shared-runtime-gossip-proof.sh
bash scripts/chat-wasm-local-smoke.sh
bash scripts/chat-wasm-native-interop-smoke.sh
bash scripts/gba-demo-smoke.sh

# Release-context only: canonical publisher signer required
just verify-release
```

### WSL

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos update
elastos setup --profile pc2
elastos setup --profile irc
elastos
```

Manual checks:
- People / identity
- Chat
- IRC
- MyWebSite
- Updates

### Jetson

```bash
curl -fsSL https://elastos.elacitylabs.com/install.sh | bash
elastos update
elastos setup --profile pc2
elastos setup --profile irc
elastos
```

Manual checks:
- People / identity
- Chat
- IRC
- MyWebSite
- Updates
