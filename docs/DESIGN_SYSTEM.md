# ElastOS Design System

This is the active visual contract for Home and the first-party browser capsules.
It is intentionally small: one shared token layer, one theme switch, one accent,
and no per-app color personality unless the app has a functional reason.

## Token Source and Vendoring

The single source of truth is `capsules/_shared/elastos-ui.css` (tokens) and
`capsules/_shared/elastos-theme.js` (theme runtime). Capsules never share these
files at runtime — `scripts/vendor-ui-tokens.sh` (run via `just vendor-ui`)
copies them into each participating capsule's browser-serving directory, and
`--check` mode fails the gate when a vendored copy drifts from the source.
Vendor targets are listed in the script; most apps serve from `browser/`,
viewer-style capsules serve from their root.

## Color Contract

Dark is the base theme. Every first-party surface consumes the `--el-*` tokens
(surfaces, text, hairlines, radii, shadows, motion) and maps any legacy local
variable names onto them in a small `:root` alias block.

- **Theme:** `elastos-theme.js` applies `data-el-theme="light"` on `<html>`
  from the `elastos.ui.theme` localStorage key (system/light/dark). The light
  palette lives in the same `elastos-ui.css`; apps must not hardcode dark
  assumptions in chrome.
- **One system accent:** `--el-accent` (plus `-ink`, `-strong`, `-soft`,
  `-faint` derivatives). The user picks one of eight system accents in
  System → Personalization (`elastos.ui.accent`, `data-el-accent` on `<html>`);
  every app obeys it and none defines its own accent color.
- **Brand:** the ElastOS orange (`--el-brand`, `#f6921a`) is reserved for
  logo-adjacent emphasis and badges — it is not the interaction accent.
- **Content is not chrome:** viewer paper (PDF/EPUB white), video letterbox
  black, and 3D stage gradients stay fixed across themes on purpose.

## Shell Chrome Anatomy

The Home shell follows a familiar desktop anatomy deliberately:

- A full-width system bar owns the focused app's name, its **menu bar**
  (see message contract below), Spotlight, inbox, identity,
  and the clock (which opens Notification Center).
- Windows carry traffic-light controls on the left; inactive windows go
  neutral. Maximized windows own the stage between bar and dock, never the bar.
- The dock is a centered pill with cosine-falloff magnification.
  Layout (left → right): **Agent** · **Apps** · pinned core · temporary
  running apps · divider · **Bin**. Running apps get an indicator dot.
- **Membership:** opening an app from Apps places a *temporary* running
  tile next to Bin (not a permanent pin). Closing the last window removes
  that tile. Permanent Shelf presence is only via drag into the pinned
  core, or an explicit Pin action. Drag from core into the running/Bin
  side unpins.
- **Motion:** membership changes breathe dock width while Bin (+ divider)
  rides with the ease; Apps face open/close is height-only (width already
  matches). Magnified icons stay overflow-visible (in front of the glass).
  Honor `prefers-reduced-motion`.

## Window Chrome Modes

Window chrome is a **presentation-only** shell mode. It never grants authority.
Unknown or missing modes **fail closed** to `standard`.

| Mode | Grammar | Opt-in |
|------|---------|--------|
| `standard` | Full titlebar: lights + icon + title; body sits below the head | Default for every target |
| `standard` + continuous | Same geometry as `standard`, but titlebar fill matches `--frame-fill` (one plate with body) | Opt-in via `WINDOW_CHROME_CONTINUOUS_TARGETS`: Wallet, Archive, GBA Emulator, wallet connectors. Not a fake sidebar. |
| `unified-sidebar` | Transparent head overlay on the **leading column only** (~220px); body is full window height; capsule pads with `--window-chrome-safe-top` | Explicit opt-in: Apps, System, Library, Services, People, Documents, **Inbox window**, **Chat / Chat Room** (People leading). Shell Inbox **rail** is a separate surface (not this map). |
| `unified-toolbar` | No window title/icon (menubar already shows focused app); transparent lights overlay on the **leading tool-row** (~76px); body full height; capsule pads `--window-chrome-safe-leading` | Explicit opt-in: Browser. Safari-inspired single tool band — not a fake sidebar. |
| `immersive` | Reserved — not shipped | — |

Rules:

- Do **not** invent in-app sidebars just to “earn” unified chrome.
- Unified head must not be a full-width opaque hit target (steals Search/main clicks).
- Capsules own safe-top padding; shell owns lights + certified drag on the leading strip.
- Materials are token + blur approximations — never claim true OS vibrancy until a Tier 3 host surface exists.
- Document every new opt-in target here and in the entropy matrix before shipping.
- Chat may temporarily force head/body geometry in shell JS when CSS tips lag
  (presentation only). `WINDOW_CHROME_BY_TARGET` remains the sole mode source —
  remove the force once tips are stable.

## Interaction Contract

Every visible action must have the same contract for humans and agents:

- A human can use it with pointer, keyboard, and readable labels.
- An agent can use the same capability-scoped API, action id, or Home message.
- DOM visibility or route shape is never authority.
- Destructive actions use in-surface confirmation and provider/runtime calls,
  not browser alerts or hidden privileged paths.
- First-party surfaces expose state in simple product nouns before raw paths,
  CIDs, or provider details.

## Home Message Contract (shell ↔ app iframes)

All messages are same-origin `postMessage`, authenticated by the app frame's
launch token (`homeToken`), validated against the frame that sent them.
Host-routed intents (`home:open-target`, `home:close-self`, …) are handled by
`capsules/home/browser/home-shell-host.js`; the presentation-only menu-bar pair
is handled by the GUI shell in `capsules/home-gui/browser/shell-menubar.js`.
The inventory:

| Type | Direction | Purpose |
|------|-----------|---------|
| `home:refresh-summary` | app → shell | ask Home to re-poll the summary |
| `home:open-target` / `home:open-uri` | app → shell | open another app / viewer (allow-listed per source app) |
| `home:deliver-to-target` / `home:open-target-with-payload` | app → shell | routed app-to-app payloads (allow-listed) |
| `home:close-self` / `home:relaunch-self` | app → shell | window lifecycle for the sender only |
| `home:menu-manifest` | app → shell | declare the app's menu bar (File/View…); UI data only, sanitized and size-capped, bound to the sender's window |
| `elastos:menu-command` | shell → app | a chosen menu item's `cmd`, posted only to the window that declared it |

Menu manifests carry zero authority: the shell renders labels via
`textContent`, commands route to the same in-app functions as the app's own
buttons, and `__`-prefixed commands (`__new-window`, `__close-window`) are
handled by the shell itself. "New Window" is offered only for targets where
`openTarget` genuinely opens a new window (never for single-session apps or
protected viewer capsules).

## Copy voice (empty / loading)

Keep status copy short and sentence case:

- Empty state = one status line + optional one action line.
  Example: “No requests” / “Approvals and invitations will appear here.”
- Loading = `Loading…` (real ellipsis `…`, never `...`), or `Loading <noun>…`
  when the noun helps.
- No policy paragraphs in empty states; details belong in docs or help.

## Shell popovers, rails, and badges

Menubar popovers (Notification Center, Control Centre) and the Wallet/Inbox
rails share one geometry contract in Home GUI CSS:

- `--popover-radius: 14px`
- `--popover-shadow: var(--shadow-soft)`

Spotlight is intentionally different: 16px radius and a heavier stage shadow —
it is a modal search stage, not a menubar popover. Do not flatten it onto the
popover tokens.

Notification badges (Inbox bell, Wallet toolbar, rail Activity) use one
formatter: `formatBadgeCount(n)` → blank for 0, raw digits through 99, then
`99+`. Dock window-count badges stay uncapped (they count open windows, not
alerts).

Inbox and Wallet rails reuse the same chassis (`wallet-rail` classes): warm
iframe, head actions, Open-window, mutual dismissal via `shell-popovers.js`.
New shell slides should clone that chassis rather than invent a third shape.

## Drift Checks

`scripts/home-entropy-check.mjs` enforces the active token set, stale-copy
removal, and the basic human/agent interaction contract for first-party browser
surfaces; `scripts/vendor-ui-tokens.sh --check` enforces vendored-token
freshness. Update this document and the checks together when the design
language intentionally changes.
