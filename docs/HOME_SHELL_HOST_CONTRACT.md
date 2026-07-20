# Home Shell Host Contract

Home has one small trusted browser host and two selectable shells:

| Identity | Purpose | Browser origin |
| --- | --- | --- |
| `home` | Front door, sign-in, shell lifecycle, and recovery | Home origin |
| `home-gui` | Desktop and window projection | Opaque sandboxed frame |
| `home-cli` | Terminal and TUI projection | Opaque sandboxed frame |

`home` is not a selectable shell. It does not render the desktop or terminal.
Runtime state selects either `home-gui` or `home-cli`, and the host mounts that
shell as the single root view.

The host lifecycle is:

`unlock -> read Runtime shell state -> launch one root shell -> route validated intents -> recover`

## Equal Shells

`home-gui` and `home-cli` are sibling shell capsules. They have the same:

- trust class and origin isolation;
- signed-in principal and Runtime-derived facts;
- launch-token validation;
- shell lifecycle and recovery rules;
- ability to request allowed capsule launches, unlock, sign-out, and an
  explicit shell change;
- prohibition on direct provider, network, wallet, storage, or Carrier
  authority.

They are not identical renderers. `home-gui` owns desktop windows and web
projections. `home-cli` owns the Runtime-owned PTY terminal and TUI projections. A
CLI action uses a CLI projection when one exists. Opening a GUI-only projection
from CLI is an explicit `switch shell and open` action; it never happens as a
side effect of running a normal CLI command.

The PTY contract uses explicit start/events/input/resize/close routes. Runtime
owns the process and stream lifecycle; `home-cli` renders bytes and sends input
or dimensions through those scoped routes.

Both shell manifests expose the common `capsule.open` and `shell.switch`
methods. Projection-specific methods such as `desktop.render` and
`facts.search` describe presentation, not additional authority.

## Origin Boundary

The Home DOM is a trusted host surface. Neither shell nor an app shares Home's
effective origin. Runtime serves every browser projection under the same
`/apps/<capsule>/` host and Home mounts it in a sandbox without
`allow-same-origin`. The browser therefore gives each frame a unique opaque
origin. This requires no wildcard DNS, extra certificate, or reverse-proxy
rule.

Opaque frames cannot read Home's DOM, session cookie, passkey surface, browser
storage, or sibling frames. Capsule state that must survive a frame belongs in
the principal-scoped Runtime namespace, not ambient browser storage. Static
assets and token-authenticated API calls may cross the opaque boundary through
the gateway's narrow `Origin: null` CORS policy; unrelated web origins remain
denied.

Runtime-generated browser routes carry a scoped launch token in the URL
fragment. The fragment is removed from visible history after bootstrap and is
never sent as an HTTP referrer. Runtime binds the token to the principal,
session, grant, target capsule, and expected browser origin. Browser API calls
must present matching Host, Origin, and Referer context. A Home session cookie
alone cannot mint an app token.

The Home session cookie is `HttpOnly`, `SameSite=Strict`, and `Secure` on HTTPS.
Capsule responses use CSP, referrer, content-type, and resource-policy headers
appropriate to their document or asset role.

## Host Responsibilities

The Home host may:

- show the passkey or guest sign-in surface as a **local account picker**
  (icon + display name for each enrolled principal) followed by that
  principal's WebAuthn ceremony — after Sign out or on a cold unsigned boot;
- read the Runtime-owned Home summary;
- launch exactly one trusted root shell;
- validate child source, origin, target, and launch token before accepting a
  message;
- mint target-scoped routes through Runtime using host-held authority;
- route Runtime events and approved payloads to their registered child frame;
- show a minimal reload, Desktop, or sign-out recovery surface.

The Home host must not:

- contain desktop, taskbar, launcher, window, or terminal implementation;
- infer authority from iframe placement or a manifest role;
- expose its session cookie or passkey result to an arbitrary frame;
- let an app or shell address a sibling frame directly;
- keep an inactive shell alive behind CSS;
- switch shells because an ordinary app-open intent was received.

## Shell Messages

Shell messages are a local browser adapter for typed Runtime intent. Every
message must come from the active shell frame, its opaque origin, and its
current launch token.

Common shell messages are:

| Message | Result |
| --- | --- |
| `home:shell-ready` | Home sends the current Runtime summary. |
| `home:refresh-summary` | Home refreshes Runtime facts. |
| `home:launch-target` | Runtime returns a target-scoped launch route. |
| `home:request-unlock` | Home displays its host-owned sign-in surface (compact re-auth for the current principal, or the full account picker on the unsigned front door). |
| `home:sign-out` | Home revokes the active session and reloads the front door (account picker). |
| `home:close-self` | Home closes the active root shell through Runtime state. |
| `home:request-unlock` (via `elastos:request-lock`) | GUI Control Centre Lock Screen asks the host to show its unlock surface; the host maps it to the existing `home:request-unlock` path. |
| `home:ui-preference` | Cosmetic prefs only (`theme`, `accent`, `dockAutoHide`, `sounds`); host persists and relays. |

Presentation-only capsule → GUI shell messages (opaque origin, no host authority):

| Message | Result |
| --- | --- |
| `inbox:pending-count` | Inbox rail marks its frame ready / syncs chrome. |
| `elastos:inbox-chrome-command` | Inbox asks the shell for rail chrome actions (open window, etc.). |
| `wallet:pending-count` | Wallet rail Activity badge sync. |

`home:switch-shell-and-open-target` is the explicit CLI-to-GUI transition for a
GUI-only target. Home validates the target, persists `home-gui` through the
Runtime shell route, launches the GUI shell, and then asks that shell to open
the target. A plain `home:open-target` from `home-cli` cannot trigger this
transition.

New active-shell writes accept only Runtime-approved `home-gui` and `home-cli`
candidates. A manifest declaring `role: shell` does not grant shell authority.

## App Messages

Apps post directly to the top-level Home host. Home registers a child only when
its source window, opaque origin, target id, and launch token all match a route
that Home obtained from Runtime.

Allowed app intents are source-scoped. Examples include opening a known target,
opening a supported object URI, delivering a typed payload to an installed
viewer, closing the current app, and requesting a fresh passkey proof for an
approved high-risk operation. Home rejects unknown sources, origins, targets,
tokens, payload shapes, and operations.

The GUI shell receives only presentation commands such as opening or closing a
window. It does not receive another capsule's authority token. App-to-app work
remains a Runtime-gated intent and, for off-box effects, follows the provider and
Carrier path described in [Capsule Interface Contract](CAPSULE_INTERFACE_CONTRACT.md).

## Passkeys And Sign-Out

Runtime owns principals, passkey records, session grants, revocation, and audit.
Home owns the browser sign-in prompt because WebAuthn is bound to the top-level
Home origin.

On an unsigned front door (cold boot or after Sign out), Home shows enrolled
local accounts from Runtime (`GET /api/auth/passkey/status` includes a minimal
`accounts` list: display name, role, credential id, optional `avatar_cid` —
never principal roots, secrets, or image bytes). The user selects an account,
then Home runs WebAuthn for that credential only. When `avatar_cid` is present,
the host loads the picture from
`GET /api/auth/passkey/account-avatar?credential_id=…` (bound to that enrolled
credential only; fail-closed to monogram). Profile pictures are set after
sign-in in **System → Accounts** (content-addressed CID on the principal
profile card), never on the unlock surface. Guest enrollment appears only when
guest registration is enabled. First boot (no passkeys) keeps the welcome →
create-admin path on the same host surface.

Only approved first-party targets may ask Home for fresh passkey authority.
Home binds that proof to the requesting capsule, operation, and request before
returning a new target-scoped token to the same source and origin.

Both shells can sign out. Browser `home-cli` sign-out is a launch-token-bound
terminal host intent; `home-gui` uses the same host message contract. Neither
shell owns or clears authentication state itself. Sign out always returns to
the host front-door account picker.

## Recovery And Cleanup

Changing shells retires the previous root frame before the next shell becomes
active. CSS hiding is not an accepted lifecycle model for a live previous
shell. Home's browser-local active-shell hint may suppress a flash during boot,
but Runtime state remains authoritative.

If the selected shell cannot launch, Home shows the host-owned recovery surface.
Recovery can retry, return to `home-gui` through the Runtime shell route, or sign
out. It does not mount a hidden desktop as a fallback.

## Verification

The minimum machine proof is:

```bash
node scripts/home-shell-bridge-smoke.mjs
node scripts/home-shell-system-switch-smoke.mjs
node scripts/home-shell-switchback-recovery-smoke.mjs
node scripts/home-cli-browser-smoke.mjs
node scripts/home-entropy-check.mjs
node scripts/home-shell-objective-audit.mjs
(cd elastos && cargo test -p elastos-server test_home_launch -- --nocapture)
```

The contract audit `first_party_capsules_have_complete_projection_contract`
keeps shell-facing catalog facts aligned. Product acceptance additionally uses
`scripts/home-shell-objective-audit.mjs --require-complete` with hash-bound
operator evidence.

For a behavior-changing shell review, create a screen-capture-free artifact
path and validate it before the objective audit:

```bash
node scripts/home-shell-manual-ux-report.mjs --template
node scripts/home-shell-manual-ux-report.mjs --notes-template --out /tmp/home-shell-manual-notes.md
node scripts/home-shell-manual-ux-report.mjs --artifact-entry /tmp/home-shell-manual-notes.md
node scripts/home-shell-manual-ux-report.mjs --report-from-notes /tmp/home-shell-manual-notes.md --out /tmp/home-shell-manual-ux.json
node scripts/home-shell-manual-ux-report.mjs --input /tmp/home-shell-manual-ux.json
node scripts/home-shell-objective-audit.mjs --manual-ux /tmp/home-shell-manual-ux.json --require-complete
```

The notes-to-report command converts the reviewed notes, requires
`source.commit` to match the candidate, and records the absence of desktop
first-paint before Home CLI. Set `redacted=true` only after reviewing the notes
for secrets.
