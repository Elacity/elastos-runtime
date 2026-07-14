# Home Shell Host Contract

This document defines the product contract for the runtime-owned Home front
door. It is the boundary between the trusted local Runtime session and exactly
two selectable Home shell surfaces: `home-gui` and `home-cli`.

The host contract is:

`unlock -> active shell selection -> mount one root shell -> route child intents -> recover on failure`

The current 0.5.0 implementation serves the host from `capsules/home`. That
path is the front door and host implementation, not a selectable shell. The
durable contract is that Home is the shell host: a Runtime-owned environment
where `home-gui` and `home-cli` are the only selectable Home shell identities.
`home-gui` is currently trusted host-loaded GUI shell code, while `home-cli` is
an isolated Runtime-owned PTY shell surface.

The shared capsule web/CLI/fact/affordance/gate/audit model is documented in
[`CAPSULE_INTERFACE_CONTRACT.md`](CAPSULE_INTERFACE_CONTRACT.md).
The current file-by-file Home ownership ledger is documented in
[`HOME_RESPONSIBILITY_MAP.md`](HOME_RESPONSIBILITY_MAP.md).

## Decided Shell Model

The current product model has three concepts, but only two selectable shell
identities:

| Concept | Final owner | Current isolation boundary | Responsibility |
| --- | --- | --- | --- |
| `ElastOS Home` / Home Host | Runtime-owned front door | Trusted host/front-door code | Unlock, session authority, active-shell state, root-shell mounting, scoped launch tokens, child-intent routing, and recovery. |
| `home-gui` | Trusted host-loaded GUI shell package with a shell manifest identity | Loaded by the Home host, not an isolated iframe/process today | Desktop, launcher, taskbar, app windows, GUI chrome, and GUI consent/projection surfaces over Home facts. |
| `home-cli` | Shell capsule | Runtime-owned PTY process plus root iframe terminal client | Terminal/TUI/CLI surface over the same Home facts, command contract, and Home-owned intents. |

`ElastOS Home` is the user environment and Runtime-owned shell host. It is not a
third shell UI and should not be presented as a selectable shell beside
`home-gui` and `home-cli`.

`home-gui` and `home-cli` are sibling shell surfaces for that same Home. They
must consume the same Runtime facts, ask through the same Home host intent
boundary, and leave authority decisions to Runtime/provider/Inbox gates. The
two surfaces do not have the same implementation boundary yet: `home-gui` is
trusted host-loaded UI, and `home-cli` is the isolated PTY shell surface.

`elastos home` is the native entrypoint for the same command-oriented Home
surface: it runs the `home-cli` capsule against the local Home snapshot. It
must not drift into a separate console-only Home product.

Final invariants:

- There are exactly two selectable Home shell identities: `home-gui` and
  `home-cli`.
- `home` is accepted only when repairing legacy saved active-shell state; new
  active-shell writes must use `home-gui` or `home-cli`.
- `/apps/home/` may remain the public Home front-door route. In the current
  implementation it is also the `home-gui` active-shell route because the GUI
  shell is host-loaded. That route is not proof of a third shell capsule or a
  separate isolated `home-gui` frame.
- Runtime routes named `/api/apps/home/...` are host/session routes unless a
  later API explicitly says otherwise.
- `elastos home` is a native entrypoint into `home-cli`, not a third Home
  implementation.

## Responsibilities

The shell host owns exactly these responsibilities:

- unlock the local principal with passkey-backed Home session authority
- read and repair invalid active-shell state for the signed-in principal
- derive selectable shell candidates from Runtime capsule catalog facts
- mount exactly one active root shell
- mint and pass scoped launch tokens to the mounted root shell
- route child shell/app intents through explicit Runtime gates
- retire, tear down, or make dormant any previous root shell surface
- provide a small recovery path when unlock, shell selection, mounting, or child
  intent routing fails

The shell host does not own product UI beyond unlock, root mounting, routing,
and recovery. Desktop, taskbar, launcher, terminal, app chrome, catalog views,
and visual layout belong to shell surfaces. In 0.5.0, the GUI shell surface is
trusted host-loaded UI; the CLI shell surface is isolated behind the
Runtime-owned PTY.

## Iframe Boundary

Home uses browser iframes as presentation containers. They are not capsule
isolation boundaries. A same-origin scripted iframe can share enough browser
authority with the host document that it must be treated as compatibility
framing, not as a sandbox.

Current same-origin iframe grants exist only where the browser surface still
depends on local Runtime APIs, EventSource, or Home host messages from the
localhost origin. The durable authority boundary is the Runtime-owned launch
token, source/target checks, provider gates, Inbox/Wallet approval, Carrier
service policy, and audit trail. Per-capsule origins or opaque-origin app APIs
are the direction for stronger browser-side separation; until then, adding a
new `allow-same-origin` target requires documenting why the target cannot run
without same-origin local API compatibility.

## Unlock

Unlock proves the human principal and establishes a short-lived Home session.

Rules:

- Passkey is the default Home unlock proof.
- A signed-in top-level Home session may read Home summary and active-shell
  state.
- State-changing shell operations require an explicit launch token, not only the
  ambient Home session cookie.
- Guest/admin policy belongs to Runtime auth and System account surfaces, not to
  shell surfaces.

Required failure behavior:

- Missing or expired authority shows the host unlock surface.
- Unlock failure does not mount a shell.
- A shell surface must not receive raw passkey ceremony state, signing keys, or
  provider handles.

Current proof:

- `shell-auth.js` owns the browser unlock surface.
- `/api/apps/home/summary` can use the Home session cookie for signed summary
  reads.
- `/api/apps/home/active-shell` writes require an explicit launch token.

## Active Shell Selection

The active root shell is a Runtime-owned per-principal setting.

Rules:

- Shell candidates come from installed capsule manifests and catalog facts.
- A selectable shell must be `role: "shell"` and launchable.
- Non-shell app capsules must not appear as active-shell candidates.
- Non-launchable or broken shell manifests must fail closed.
- Final-state active-shell writes use `home-gui` or `home-cli`, never `home`.
- Invalid, missing, or no-longer-launchable active-shell state repairs to
  `home-gui`.
- UI surfaces may request a switch, but they do not make a shell authoritative.

Identifier boundaries:

| Current id/name | Final meaning | Status |
| --- | --- | --- |
| `home` | Home host/front-door id, `/apps/home/` route, and `/api/apps/home/...` host namespace. | Host-only. Not a shell candidate. Repaired only from legacy saved active-shell state; rejected as a new active-shell write. |
| `home-gui` | GUI shell identity and host-loaded package. | Final selectable shell name. Canonical active-shell id backed by `capsules/home-gui`; owns the desktop/window implementation but currently runs inside the trusted Home host context. |
| `home-cli` | CLI shell capsule identity. | Final selectable shell name. Isolated through the Runtime-owned PTY/browser terminal path. |

`home` can remain in route, API, profile, command, or environment names only
when the referent is the Home host or operator entrypoint. If legacy saved
active-shell state says `home`, Runtime repairs it to `home-gui` and persists the
canonical name. New active-shell writes use only installed launchable shell
candidates.

## Mount One Root Shell

The shell host mounts one active root shell at a time.

Rules:

- The active root shell is mounted in the host-owned root mount point.
- The mounted root shell receives a scoped launch token in its launch route.
- Switching to an alternate root shell retires live GUI shell windows.
- Previous shell UI must not keep running invisibly behind the active shell.
- CSS hiding is not an accepted lifecycle model for a live previous shell.
- Root shell mounting must be cache-busted with Home browser assets.

Current implementation:

- `#active-shell-root` is the host-owned root mount point.
- `capsules/home-gui/browser/home-gui.js` is the current GUI shell facade for
  instantiating the inert `home-gui` template, desktop mounting, dormant-state
  teardown, session restore, GUI controls, desktop input, desktop appearance,
  window lifecycle binding, and GUI rendering. It is lazy-loaded by the Home
  host as trusted code; it is not currently mounted as a separate root iframe,
  process, or VM.
- GUI chrome projection belongs behind the GUI facade too: toolbar clock,
  identity surface, Inbox badge, and Wallet approval toast are `home-gui`
  responsibilities. `home-shell-host.js` must not statically import the GUI
  chrome/surface modules or run those projections during alternate-shell summary
  refresh.
- The host may mark the GUI shell dormant as lifecycle state, but it must not
  query or mutate desktop/taskbar/launcher DOM nodes itself. GUI node creation,
  hiding, inertness, clearing, binding, and rendering are owned by the `home-gui`
  facade; the host page starts without desktop GUI markup and lazy-loads the
  `home-gui` template only when that shell is actually mounted.
- GUI-only templates for windows, desktop shortcuts, launcher items, taskbar
  items, and window errors live in the lazy `home-gui` template. They must not
  ship in the first host document.
- Home Host summary handling must not require GUI DOM or GUI layout state.
  Desktop layout normalization, browser-window session state, glyphs, and GUI
  surface state live in `capsules/home-gui/browser/shell-core.js` and run only
  when `home-gui` is mounted.
- When `home-cli` becomes active, Home marks the GUI shell dormant, removes live
  GUI windows, clears desktop/taskbar entries, makes GUI chrome inert, and lets
  `home-cli` own the viewport.
- `home-shell-host.js` lazy-loads the GUI facade only when `home-gui` is the
  active shell. If an alternate root shell asks the host to open a normal GUI
  app window, it must emit an explicit GUI-open host intent such as
  `open-gui:<target>` with the shell launch token; then `home-gui` owns the
  window. Ordinary dynamic `capsule-*` actions must not trigger this switchback
  by default. The alternate-shell first-paint path must not statically boot
  desktop input, launcher, taskbar, window behavior, or GUI CSS.
- Home and System may store a browser-local active-shell hint to suppress
  desktop first paint on the next load. The hint is non-authoritative: Runtime
  summary and launch tokens still decide the active shell and capsule route.
- After System writes a new active shell through Runtime, it may send a signed
  `home:active-shell-applied` intent to the host. The host may use that intent
  only to pre-retire stale GUI surfaces and show the neutral root-shell mask;
  the next `/api/apps/home/summary` still decides the authoritative shell.
- The host keeps a neutral boot mask over the front door until Runtime selects
  a shell and the host either attaches the alternate shell, mounts `home-gui`,
  shows neutral auth, or shows host recovery. The mask is not a fallback shell
  and must not carry desktop wallpaper, icons, taskbar, launcher, or app state.
- When `home-gui` becomes active, the host clears the alternate root mount and
  restores GUI shell rendering.
- Window session restore is owned by `home-gui`. Sessions without a `root_shell`
  owner are not restored, and `home-cli` does not own GUI window sessions.
- `home-cli` is terminal-only in the browser product path. On launch it attaches
  a Runtime-owned PTY terminal and renders it with capsule-local xterm.js.
  Runtime owns the process, PTY, stream ticket, launch-token gate, dimensions,
  input/resize routes, and lifecycle; `home-cli` sends raw terminal input
  without receiving host process authority.
- The Runtime terminal uses xterm.js for cursor, scrollback, selection,
  alternate-screen, ANSI stream rendering, resize, paste, mouse events, and
  Home CLI TUI input. The `home-cli` TUI accepts keyboard navigation plus SGR
  mouse wheel movement and tab-row clicks through that PTY. No browser-side
  command projection remains: the browser wrapper must not fetch ESP/catalog
  facts, render a command form, interpret shell commands, or launch app routes.
  It only starts the Runtime PTY, renders bytes, sends terminal input/resize,
  forwards structured Home-owned host intents emitted by `home-cli`, and
  returns to `home-gui` when the Runtime terminal lifecycle ends.
- `capsules/home-cli/browser/commands.json` is the shared Home CLI command contract.
  The `home-cli` binary embeds it for line-mode help and the
  snapshot-backed commands it can honestly serve; the browser wrapper does not
  consume this file.
- Home CLI line mode keeps the default product vocabulary small:
  `home`, `apps`, `invoke`, `inbox`, `wallet`, `exits`, `refresh`, `help`, and
  `exit`. Developer projections are explicit `debug ...` topics such as
  `debug capsules`, `debug inspect <capsule>`, `debug affordances [capsule]`,
  `debug gates [capsule]`, `debug audit <capsule>`, `debug people`,
  `debug spaces [root]`, `debug services`, `debug browser`,
  `debug terminal`, and `debug contract`. It reads serialized
  `elastos.capsules.catalog/v1`, `elastos.capsules.interfaces/v1`, and
  `elastos.runtime.services/v1` facts from `snapshot.json`; it does not call
  Home HTTP routes, provider routes, System routes, or browser storage directly.
  Low-risk `invoke` resolves Runtime interface facts and writes a structured
  `intent.json`; the Home owner process then mints a non-delegatable launch
  token and calls `/api/capsules/interfaces/invoke`. The Home CLI test suite
  covers the serialized invoke payload so this remains a Home intent boundary,
  not a local provider-dispatch path.
- `home-cli` renders capsule gate questions from the same interface facts used
  for affordance inspection: `gates <capsule>` shows declared method risk,
  approval, gate, audit, resource, and operation metadata where present. These
  rows are descriptors, not grants.
- `home-cli inspect <capsule>` renders Runtime-derived catalog projection facts
  for web, CLI, facts, affordances, gates, audit/mirror, and Carrier/service
  readiness. It must not infer these surfaces from local UI code.
- `home-cli invoke <capsule> <method> [json|target]` resolves declared
  affordances from the Runtime interface registry and blocks user/high-risk
  methods before dispatch. The `home-cli` binary writes the structured invoke intent
  for the Home owner process to dispatch; in browser root mode the user reaches
  that same binary through the Runtime PTY. The Runtime route still
  enforces target-token authority, audit, and bound-handler policy.

Current ownership:

- `capsules/home` is the Runtime-owned Home host/front-door implementation. It owns
  unlock, session refresh, active-shell resolution, root-shell mount,
  launch-token routing, child-message policy, and host recovery.
- `capsules/home-gui` is the trusted host-loaded GUI shell package with a
  shell-role manifest. It owns desktop, launcher, taskbar, window/app chrome,
  GUI template, GUI layout state, browser-window session state, and GUI
  rendering. Its current route is the Home host route, `/apps/home/`.
- A future isolated `home-gui` would need a separate root-shell attach path,
  authority handoff, recovery proof, and tests before this document may call it
  an isolated shell capsule.
- Keep `/apps/home/` as the public front door.
- Keep root shell lifecycle policy in the host, not in `home-gui` or
  `home-cli`.

## Risk Controls

- **Two-shell boundary.** Only `home-gui` and `home-cli` are selectable shells.
  `home` stays a host route/name and legacy saved-state migration value only.
- **Host/UI separation.** The host owns unlock, root mounting, launch-token
  routing, and recovery. Shell capsules own desktop or terminal UI.
- **State repair.** Invalid, missing, or no-longer-launchable active-shell state
  repairs to `home-gui` through the generic invalid-state path; legacy `home`
  state repairs to the same canonical `home-gui` identity.
- **Token boundary.** Shell capsules never mint tokens, dispatch providers,
  handle passkey ceremony state, or own authority-bearing policy.
- **Manual proof.** Shell switching touches passkey, recovery, app launch, and
  browser reload behavior. Manual proof must be regenerated after each
  meaningful Home shell change.

## Route Child Intents

Shells and apps emit intents. The host routes them only after validating source,
token, target, and policy.

Accepted intent categories:

- refresh summary
- open a visible target
- open an allowed `elastos://` URI through its viewer
- deliver a typed payload to an already-open allowed target
- close or relaunch the calling app frame
- ask the host to return from an alternate root shell to `home-gui`

Rules:

- Child messages must come from the same origin and from a known frame.
- Child messages must carry the launch token that matches the sender frame route.
- A root shell frame is treated differently from an app window frame.
- Root shell frames that emit app-open messages must use the frame launch token.
  The host first uses that token to return to `home-gui`, then opens the target
  as a normal `home-gui` window. The current browser `home-cli` wrapper does not
  emit those messages itself; it renders the Runtime PTY and lets `home-cli`
  write Home-owned intents.
- Shells do not call provider APIs directly for authority-bearing effects.
- Provider dispatch, wallet signing, Inspector acts, and approval flows stay
  behind Runtime/Provider/Inbox gates.

The current HTTP routes are browser-host adapters. The durable contract is:

`shell capsule -> runtime capability/session -> ESP facts/intents -> runtime gate -> provider/Carrier plane`

Future Carrier transport must preserve the same schemas, gates, consent path,
dispatch path, and audit semantics.

## Recover On Failure

Recovery is host-owned because shells can fail before they can render their own
controls.

Required recovery states:

- unlock unavailable or expired
- active-shell state invalid
- selected shell is no longer installed or no longer launchable
- root shell launch fails
- mounted shell route returns an unsupported attach kind
- child intent is malformed, unauthorized, or targets a missing capsule
- active shell crashes or cannot be rendered

Required recovery behavior:

- Fail closed and do not mint broader authority.
- Prefer repair to `home-gui` when the saved shell state is invalid or points to
  a shell that is no longer launchable.
- Show a minimal host-owned recovery surface with reload, sign out, and switch
  back to `home-gui`.
- Do not reuse the full `home-gui` toolbar as the only recovery surface.
- Log enough detail for operator review without exposing secrets in the shell.

Current recovery status:

- Invalid active-shell state repairs to `home-gui`.
- Alternate root mount failure shows a host-owned recovery surface.
- The recovery surface can reload, sign out, and switch back to `home-gui` when
  a shell launch token is available. Without that explicit token it fails
  closed and asks the user to reload or sign out.

## Security Invariants

- Shells are projection and consent surfaces, not authority layers.
- Active-shell writes require explicit launch tokens.
- Ambient Home cookies may read signed summary state but must not switch shells.
- Only one root shell may be active.
- Previous GUI shell windows must be retired or hibernated, never merely hidden.
- Shell candidates are Runtime catalog facts, not local UI lists.
- Child intent routing validates origin, frame, route token, source capsule, and
  target policy.
- ESP facts are read-only projections. They are not grants.
- Route-specific Runtime gates remain the gates.
- Provider effects go through ProviderRegistry/Provider contracts and audit.
- Future off-box shell/exit behavior must go through Carrier/provider contracts,
  not direct capsule-to-capsule browser state.

## Two-Shell Verification Gates

Before claiming the final two-shell architecture, prove all of these:

- Manifest/catalog gate: installed launchable shell candidates are exactly
  `home-gui` and `home-cli`; `home` is not a shell candidate.
- State gate: writing active shell persists only `home-gui` or `home-cli`;
  invalid saved state repairs to `home-gui`; legacy saved `home` state repairs
  to `home-gui`; new `home` writes are rejected.
- Host gate: `/apps/home/` still unlocks and mounts exactly one selected root
  shell without owning desktop, taskbar, launcher, terminal, or app chrome UI.
- GUI gate: selecting `home-gui` renders the desktop and restores only
  `home-gui`-owned windows/session state.
- CLI gate: selecting `home-cli` renders the Runtime-owned PTY terminal without
  instantiating `home-gui` DOM or painting desktop first.
- Switch gate: System and shell-origin switch requests require explicit launch
  tokens; ambient cookie-only writes still fail.
- Intent gate: explicit GUI app opens from `home-cli` return through the Home
  host intent path and do not directly launch app routes from the shell.
  Ordinary dynamic `capsule-*` actions stay in the CLI launch matrix and must
  not switch to `home-gui` implicitly.
- Recovery gate: broken/missing `home-gui` or `home-cli` manifests fail closed
  with host recovery, not fallback shells.
- Native gate: `elastos home` runs the same `home-cli` capsule command contract
  and does not introduce a third console Home implementation.
- Entropy gate: docs, tests, manifests, and UI copy contain no selectable
  third-shell language for `home`.

## Current Verification

Focused bridge proof:

```bash
node scripts/home-cli-browser-smoke.mjs
cargo test --manifest-path capsules/home-cli/Cargo.toml command_contract -- --nocapture
cargo test --manifest-path capsules/home-cli/Cargo.toml home_cli_line_mode_accepts_shared_snapshot_backed_commands -- --nocapture
cargo test --manifest-path capsules/home-cli/Cargo.toml home_cli_line_mode_reads_browser_exit_service_offers -- --nocapture
cargo test --manifest-path capsules/home-cli/Cargo.toml home_cli_line_mode_serializes_structured_invoke_home_intent -- --nocapture
(cd elastos && cargo test -p elastos-server first_party_capsules_have_complete_projection_contract -- --nocapture)
node scripts/home-shell-auth-gate-smoke.mjs
node scripts/home-shell-bridge-smoke.mjs
node scripts/home-shell-no-hint-boot-smoke.mjs
node scripts/home-shell-stale-hint-boot-smoke.mjs
node scripts/home-shell-recovery-smoke.mjs
node scripts/home-shell-switchback-recovery-smoke.mjs
HOME_URL=http://localhost:61180/apps/home/ node scripts/home-passkey-virtual-auth-smoke.mjs
node scripts/home-shell-objective-audit.mjs
```

This proves the current bridge behavior:

- alternate shell launches in root mode
- the active shell root mounts `home-cli`
- a remembered alternate-shell hint lets the root shell claim first paint before
  `/api/apps/home/summary` returns, without launching a frame before Runtime
  authority
- when the remembered hint is absent, the host stays in a neutral resolving
  surface, keeps GUI chrome hidden through Runtime ensure, refreshes the
  authoritative shell summary, then launches `home-cli` without binding desktop
  input
- if a remembered alternate-shell hint meets an initial stale `home` summary,
  the host keeps `home-gui` dormant through Runtime ensure, then follows the
  next Runtime summary instead of using the hint as shell-switch authority
- the neutral boot mask prevents stale or desktop-flavored first paint until the
  selected shell, neutral auth gate, or host recovery surface is visible
- CLI boot does not instantiate the `home-gui` template, so there is no live
  desktop backdrop, toolbar, launcher, or taskbar to hide behind it
- `home-gui` is marked dormant
- stale GUI windows are removed
- desktop/taskbar entries are cleared
- GUI chrome is inert
- window restore rejects sessions owned by another root shell
- auth prompts retire the stale root shell, app windows, and desktop chrome
  before showing the passkey card, then use a neutral host surface instead of
  blurring desktop UI
- browser `home-cli` never launches app routes directly; the `home-cli` TUI
  writes a Home-owned `intent.json`, and gateway-owned terminal app-open intents
  are forwarded to the Home host through the signed shell-frame message contract
- embedded `home-cli` does not launch app routes directly
- shell-frame app-open intents with the wrong origin, the wrong route token, or
  a request to open the host route or `home-gui` are ignored before any launch
  request, shell switch, or GUI window is created
- the smoke does not switch shells with ambient active-shell state
- failed root-shell launches show the host recovery surface
- recovery does not switch shells with ambient active-shell state
- signed switchback failures use the mounted shell launch token, then show the
  host recovery surface without mounting `home-gui`
- a signed virtual-passkey Home session can use System's shell picker to switch
  to `home-cli`
- System's signed active-shell-applied message immediately retires `home-gui`
  and cancels stale root-shell launches before the authoritative summary
  refresh relaunches the selected shell
- `home-cli` fills the root viewport, hides dormant GUI chrome, and returns to
  `home-gui` from the CLI
- the `home-cli` TUI can launch Browser through a Home-owned intent; Home
  returns to `home-gui` with the shell launch token, then creates a normal
  Browser window through the signed host intent path
- `home-cli` can answer `gates <capsule>` from Runtime interface facts without
  calling provider or System routes directly
- `home-cli` can attempt runtime-policy affordance invocation through
  `/api/capsules/interfaces/invoke`, carries its launch token, and blocks
  user-approval methods before dispatch
- browser Runtime-PTY `home-cli` and `elastos home` consume the same
  `commands.json`; Home CLI line mode accepts the read-only subset backed by
  its Home snapshot, serializes
  low-risk `invoke` as a structured Home intent, and still blocks high-risk
  affordance invocation before dispatch
- `elastos home` / `home-cli` can show Wallet request hints, Browser target facts, and
  Browser Exit service offers from Runtime snapshot facts without opening
  provider or System routes directly
- the Runtime catalog read model projects every visible first-party capsule with
  web, CLI, facts, affordances, gates, audit/mirror, and Carrier surfaces, and
  the interface registry counts match the catalog

Current active-shell authority proof:

```bash
(cd elastos && cargo test -p elastos-server test_home_active_shell_uses_catalog_shell_candidates -- --nocapture)
```

This proves:

- shell candidates come from launchable shell catalog facts
- non-shell apps and broken shells are rejected
- invalid saved shell state repairs to `home-gui`
- System and shell launch tokens can switch shells
- cookie-only active-shell writes are forbidden

Current active-shell identity proof:

- Catalog facts and shell picker projections expose `home-gui` and `home-cli`
  only.
- shell candidates are exactly `home-gui` and `home-cli`
- `home` does not appear as a selectable shell candidate
- legacy saved `home` active-shell state repairs to `home-gui`
- new `home` active-shell writes are rejected
- active-shell writes persist `home-gui` or `home-cli`, never `home`
- `home-gui` mounts through the trusted Home host facade, while `home-cli`
  mounts through an explicit launch-token-gated root iframe and Runtime PTY
- cookie-only active-shell writes remain forbidden

Root-shell browser smoke proof:

```bash
node scripts/home-shell-system-switch-smoke.mjs
```

Operator-profile manual proof:

```bash
node scripts/home-shell-manual-ux-report.mjs --template --out /tmp/home-shell-manual-ux.json
node scripts/home-shell-manual-ux-report.mjs --notes-template --out /tmp/home-shell-manual-notes.md
node scripts/home-shell-manual-ux-report.mjs --artifact-entry /tmp/home-shell-manual-notes.md
node scripts/home-shell-manual-ux-report.mjs --report-from-notes /tmp/home-shell-manual-notes.md --out /tmp/home-shell-manual-ux.json
node scripts/home-shell-manual-ux-report.mjs --input /tmp/home-shell-manual-ux.json
node scripts/home-shell-objective-audit.mjs --manual-ux /tmp/home-shell-manual-ux.json --require-complete
```

The template is intentionally rejected until a human fills every check on the
real operator browser profile and attaches at least one redacted, hash-bound
artifact. The notes template is the screen-capture-free artifact path; the
artifact-entry helper computes the SHA-256 but keeps `redacted=false` until a
human has reviewed the note file for secrets. `--report-from-notes` converts the
filled notes into the JSON report only after every required check has evidence
and the note file says it was reviewed for secrets. The proof records passkey
sign-in, System shell switch, CLI fullscreen, absence of hidden GUI chrome
bleed-through, absence of desktop first-paint before CLI, switching back to
`home-gui`, Browser/GBA staying blocked from default CLI actions unless an
explicit GUI-open intent is offered, and reloads without the passkey loop.

The objective audit also requires the manual report's `source.commit` to match
the current `HEAD`; after any Home shell change, regenerate or re-review the
manual notes before claiming completion.

The objective audit is also intentionally fail-closed until the terminal half of
the objective is real. The current Home CLI has a terminal-only browser product
surface backed by an xterm-rendered Runtime-owned PTY contract.
`--require-complete` must still not pass without accepted operator-profile
manual UX evidence.

Required handoff gate:

```bash
git diff --check
node scripts/home-entropy-check.mjs
(cd elastos && cargo fmt --all -- --check)
cargo fmt --manifest-path capsules/chain-provider/Cargo.toml -- --check
```

## Not Yet Claimed

The current branch does not yet claim:

- a fully extracted `home-shell-host` capsule or crate
- a true isolated `home-gui` root-shell iframe, process, VM, or separate
  authority handoff
- accepted operator-profile manual UX evidence for final second-shell product
  readiness
- shell marketplace
- standing grants
- reach/egress enforcement
- product-ready ESP SSE streams

Those items require their own implementation slices and proof before the docs or
UI may describe them as product-ready.
