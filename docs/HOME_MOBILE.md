# Home on phones and tablets

This is the design charter for the Home GUI shell (`capsules/home-gui`) on
coarse-pointer devices. It says what a phone or tablet host must feel like,
how the shell decides which layout it is in, how the result is measured, and
what a first-party capsule has to do to fit inside the phone stage.

It does not change what Home is. One shell, one capsule contract, one set of
gates. A phone is a host adapter for the same Home (see
[ROADMAP.md](../ROADMAP.md), "Host adapters for server, desktop, mobile, and
kiosk deployments must preserve the same Home and capsule contracts").

## Size classes

The shell recognises three size classes. Width is the CSS viewport width of
the Home GUI frame.

| Class | Width | Pointer | Layout |
| --- | --- | --- | --- |
| phone | ≤ 640 px | usually coarse | app-grid Home, one window at a time, sheets instead of popovers, 44 px chrome |
| tablet | 641–1100 px | coarse or fine | desktop layout with 44 px chrome when the pointer is coarse |
| desktop | > 1100 px | usually fine | the existing desktop |

Pointer class comes from `(pointer: coarse)` and `(hover: none)`, never from
the user agent string. A tablet with a trackpad is a fine-pointer tablet; a
touch laptop under 640 px is a phone.

A phone held sideways is still a phone: a coarse-pointer viewport whose
height is at most 640 px keeps the phone class (844 × 390 is a phone, not a
tablet), so rotating with a window open does not change the layout family. A
short desktop window with a mouse stays desktop; height only counts on a
coarse pointer.

The single source of truth for the class in JavaScript is
`shell-form-factor.js`; `shouldOpenMaximizedByDefault()` in `shell-core.js`
delegates to it, and the class is mirrored onto
`document.body.dataset.formFactor` and `dataset.pointer` so CSS and tests read
the same fact. New phone rules key on `body[data-form-factor="phone"]`, not
on `@media (max-width: 640px)`, so landscape follows.

## Viewport truth (M1)

- Both `index.html` files declare `viewport-fit=cover` (safe areas become
  real on iOS) and `interactive-widget=resizes-visual` (Android Chrome shrinks
  only the visual viewport for the soft keyboard, like iOS).
- Only the top-level document's visual viewport shrinks for the soft
  keyboard; a framed document's visual viewport is its layout viewport. So the
  host page measures the covered height (`home/browser/home-keyboard-inset.js`)
  and relays it to the shell as `home:keyboard-inset`. `shell-form-factor.js`
  takes it only from the trusted parent and writes `--keyboard-inset` on
  `:root` on coarse-pointer hosts; `--stage-bottom` adds it and the Assistant
  room ends above it, so an open window and the Assistant composer stay above
  the keyboard. Desktop pinch-zoom never produces an inset. The phone smoke
  plays the host in both engines.
- `overscroll-behavior: none` on `html, body`; `touch-action: manipulation`
  and no tap highlight on shell controls; `-webkit-touch-callout: none` on
  shell chrome only, never on capsule frames.
- Every `100vh` carries a `100dvh` twin on the next line.
- The bar pads its ends with `env(safe-area-inset-left/right)`; the phone
  window inset does the same, so landscape notches never cover content.

Deferred from M1 on purpose: converting the shell's 66 `px` font sizes to
`rem`. iOS Safari does not scale `rem` with Dynamic Type (only
`-apple-system-body` fonts scale), Android Chrome does. The phone layer (M4)
declares its own sizes and will use `rem` there; changing every desktop size
in one sweep would touch dozens of pinned visual contracts for a benefit
limited to Android.

## Rubric

Every shell surface is graded on the same five things, in this order.

1. **Targets.** Every interactive control is at least 44 × 44 CSS px
   (Apple HIG, WCAG 2.5.5). Glyphs may stay small; the hit box grows.
2. **Text.** No shell text under 12 px on a phone. Shell text sizes are in
   `rem` so OS text-size settings apply (WCAG 1.4.4); layouts hold at 130 %.
3. **Reach.** Primary actions sit in the lower half of the screen; nothing
   the person must tap lives under the notch or the home indicator.
4. **Grammar.** Every gesture has a button twin. Long-press opens the same
   menu right-click opens. Swipe-down on a title bar opens the app switcher
   and keeps the app running; Control Centre → Overview is its button twin.
   The trailing Minimise chevron sends the app Home, still running; the
   leading Close closes it. No horizontal edge gestures anywhere in the shell.
5. **Truth.** Safe areas are real (`viewport-fit=cover`), the soft keyboard
   never covers a focused input (host `visualViewport` → `--keyboard-inset`),
   `100dvh` not `100vh`, no rubber-band overscroll on the shell.

Capsules are graded on 1, 2 and 5 inside the phone stage; 3 and 4 are shell
responsibilities.

## Gesture surface rule

Capsules run in opaque sandboxed iframes and own every touch that lands on
them. A shell gesture may start only on shell chrome: the title bar, the dock
handle, a sheet handle, the desktop. The shell never listens for a swipe
that begins over capsule content, and never uses a horizontal edge gesture,
because Android back and iOS back both live there. The Home pages turn with
a swipe across the grid, not from the edge; the edit mode's page turn is a
dragged icon held near the edge, not a swipe from it.

## How it is measured

`scripts/home-phone-layout-smoke.mjs` runs in `just product-ui-browser` (the
`verify-release` lane). It loads the real Home host and Home GUI source
against a fixture host — no Runtime, no passkey — in Playwright Chromium and
WebKit at three profiles:

| Profile | Viewport | DPR |
| --- | --- | --- |
| phone-portrait | 390 × 844 | 3 |
| phone-landscape | 844 × 390 | 3 |
| tablet | 820 × 1180 | 2 |

For the desktop (the Home grid on the phone profiles), the launcher (tablet
only), the app menu, Spotlight, Control Centre, Notification Centre,
Mission Control, the Assistant face and one window per first-party app it
records targets under 44 px, text under 12 px and horizontal overflow, plus
the source truths that need no browser (`viewport-fit=cover` in both
`index.html` files, bare `100vh` count, `backdrop-filter` count). It writes
`report.json` and a screenshot per surface to `HOME_PHONE_SMOKE_OUT` (default
a temp directory).

The gate is a ratchet. `BASELINE` in the script pins today's numbers per
profile and surface; a run may not exceed them. A PR that improves a surface
lowers its row in the same commit. Nothing raises a row.
`CAPSULE_TARGET_BASELINE` is the same ratchet inside each app's own frame
(and the Assistant's, measured when its face opens) for targets under 44 px
on the phone profiles. M6 took every row to 0, so a capsule change that adds
a small target fails the smoke.

```bash
HOME_PHONE_SMOKE_OUT=/tmp/phone-smoke node scripts/home-phone-layout-smoke.mjs
# one engine while iterating
HOME_PHONE_SMOKE_ENGINES=chromium node scripts/home-phone-layout-smoke.mjs
```

WebKit is required for the release lane because iPhone Safari is the phone
engine. If the WebKit build for the bundled Playwright is missing, run
`npx playwright install webkit` inside `elastos/tools/browser-playwright-engine`.

Source truths that the smoke measures are also pinned by
`scripts/home-entropy-check.mjs` in `just verify`, one assertion per truth,
so a phone contract cannot drift without a gate going red even on a machine
without browsers.

## Bar and Dock (M2)

CSS keys the phone size class on one media list that mirrors
`classifyFormFactor()`: `(max-width: 640px)`, or a coarse/hover-less pointer
with `(max-height: 640px)`. A phone held sideways therefore keeps the phone
shell. The entropy check derives the list from `PHONE_MAX_WIDTH`, so the two
readers cannot drift apart.

Bar:

- `--toolbar-h: 44px`. Every bar control is a 44 × 44 hit box (11 px padding
  around the same 22 px glyph); the clock's line-height is the bar height;
  the inbox badge is 16 px with 12 px numerals.
- Composition on phone is brand · inbox · search · status · clock. Wallet and
  Overview leave the bar; Control Centre keeps both one tap away (Quick open
  and Overview rows), so nothing is lost. The phone smoke opens Mission
  Control through that route.
- The bar pads its trailing edge with `env(safe-area-inset-right)` only; the
  44 px boxes supply the visual margin.

Dock:

- The pinned row scrolls with the thumb (`touch-action: pan-x` on tiles; the
  desktop reorder drag still works because Playwright/pointer cancel ends the
  drag), snaps tiles to slots, and fades only the edge that hides more tiles
  (`--dock-fade-start/end`, written on scroll by `shell-phone-dock.js`).
- The pill keeps `env(safe-area-inset-left/right)` clear in landscape.
- The running indicator is a 14 × 4 px capsule.
- With a window open the Dock tucks off screen (`body.phone-dock-tucked`) and
  `--stage-bottom` shrinks to a 24 px handle above the home indicator, so the
  app gets the stage. Tap or swipe the handle up to peek the Dock over the app
  (`body.phone-dock-peek`, scrim behind it); tapping the scrim, pressing
  Escape, or picking a window tucks it again. Mission Control and keyboard
  focus inside the Dock always show it. The desktop
  auto-hide preference is untouched; a peek wins over it.
- `syncPhoneDock()` runs from `updateTaskbarState()`, the one place every
  window open, close and focus already passes through, and on resize.

The smoke probes the tuck → peek → tuck cycle on both phone profiles and
fails if the window does not reach the handle or the handle is under 24 px.
`scripts/home-phone-dock.test.mjs` covers the decision logic under
`node:test`.

The 74 KB `wallpaper.webp` is already smaller than a phone-specific crop
would be; no `image-set()` variant.

## Window title bar (M3)

- One 44 px presentation of the existing `.window-head` for every chrome
  mode: leading Close (✕), centred capsule icon + title, trailing Minimise
  drawn as a chevron — on a phone minimise means "go home", the capsule keeps
  running and the Dock returns. Fullscreen is hidden; the stage is already
  the whole screen. `WINDOW_CHROME_BY_TARGET`, `parseWindowChromeMode` and
  `applyWindowChrome` are untouched; the traffic-light group becomes
  `display: contents` so the same three buttons land in a three-column grid.
- The head is in flow above the body in all modes, including unified
  sidebar/toolbar, because a phone-width capsule has content at the very top
  of its main column and an overlay would cover it. Capsules still pad their
  own 52 px / 96 px safe areas for the desktop; lifting that per capsule on
  phone is M6.
- The title bar is on screen the instant the window opens (the title comes
  from the summary, not the capsule). A 2 px accent hairline runs under it
  until `.window-frame.is-ready`; under `prefers-reduced-motion` it is a
  static line.
- `shell-window-geometry.js` guards drag and resize on phone (focus still
  follows the touch); nothing is deleted, the desktop maths is unchanged.
- The continuous-chrome windows (Wallet, Archive, GBA, connectors) already
  obey the stage through the generic phone `.window` rule.
- Open: the window rises from the Dock in 200 ms (`phone-window-enter`,
  transform + opacity only); none under `prefers-reduced-motion`.

## App switcher, swipe and system back (M3)

- Mission Control is the phone's app switcher. A downward drag on the title
  bar (≥ 48 px, cancelled by ≥ 32 px of sideways drift) opens it; the drag
  starts on the title bar only, because capsule frames own their touches
  (`shell-phone-stage.js`, `TITLE_SWIPE_DOWN_PX`, `TITLE_SWIPE_DRIFT_PX`).
  The head takes pointer capture so the moves keep arriving once the finger
  crosses into the capsule frame, and the click that ends the gesture is
  swallowed — it would land on the card and undo the open. Control Centre →
  Overview opens the same switcher by button; the Minimise chevron sends the
  window Home instead.
- Cards carry the capsule icon and name (`.expose-caption`, counter-scaled by
  `--expose-card-scale` so they read at natural size) and, on coarse
  pointers, a 44 px Close (`.expose-close`) — the scaled title bar's own Close
  is no longer a target. Touch cards let both hang outside the scaled window
  (`overflow: visible; contain: none`). Portrait stages size the Space thumbs
  by height (`MISSION_PORTRAIT_THUMB_MIN_W`); the landscape width floor made a
  portrait thumb ~350 px tall and left the cards a quarter of the screen.
- Copy on coarse pointers: "Tap + for a new Desktop…", no mouse language.
- Stage history (`shell-phone-stage-history.js`, pure, unit-tested): the
  shell keeps at most one entry of its own, pushed when the first layer (a
  window, a sheet, Mission Control) opens and consumed when the last closes,
  so back on the bare desktop leaves Home as before. popstate closes the top
  layer: the shell's Escape registry, then Mission Control, then any open
  sheet through its own closer, then the window goes home (minimise). The
  decision reads `history.state` — capsule frames add joint-session steps of
  their own above ours and traversal is asynchronous, so counting pushes
  drifts — and never issues more than one `back()` per close cycle: a second
  one before the first lands could walk out of the host's history.
- **WebKit fallback (recorded per plan).** Safari and every iOS browser
  record nested frame loads as joint steps and, seen from the shell frame,
  one `history.back()` after a `pushState` took the shell frame itself to
  `about:blank` in the phone smoke (2026-09-23). `stageHistorySupported()`
  therefore keeps stage history off on WebKit (`body[data-stage-history]` is
  `buttons` there, `history` on Chromium/Gecko). Every function stays
  reachable by button and swipe; iOS has no system back button, Android
  (Chromium) gets it. Revisit if the frame loads move to
  `location.replace()`.
- The phone smoke probes all of it on the first window: swipe → switcher
  with caption icon and Close, `history.back()` → home with no entry left
  (Chromium) or nothing pushed (WebKit), the Home frame alive either way, and
  rotation to landscape and back with the window still filling the stage.

## Sheets (M4)

- Launcher: superseded on the phone by the Home grid (below), which retired
  the phone Apps sheet. On tablet and desktop it stays browse-only: opening
  never focuses a field, so a touch tablet's keyboard does not jump up;
  typed search is Spotlight's.
- Spotlight hangs 8 px under the bar and grows only to the Dock or the soft
  keyboard (`--stage-bottom` carries `--keyboard-inset`). Result rows are
  48 px with 16 px names and 12 px section headings. A finger landing on a
  row opens it on click, not on press, so scrolling the results never
  launches an app; a mouse still opens on press.
- Control Centre: 48 px rows, 12 px labels, 51 × 31 switches, 44 px segment
  options and accent swatches (the swatch paints 28 px inside a transparent
  border), Quick open shown.
- Notification Centre: calendar weekdays 12 px, days 34 px at 15 px,
  section heading and times 12 px, Clear history a 44 px target.
- Reduced motion stops the launcher's opening fade, blur and scale as well
  as its height change.
- Grab handles (`shell-sheet-handle.js`): Control Centre, Notification Centre
  and Spotlight end in a 44 px Close handle at the thumb end; a tap closes, a
  48 px drag up closes, a shorter drag snaps back. Every sheet also closes
  on Escape and on system back.
- The bar sheets are non-modal dialogs (`role="dialog"`,
  `aria-modal="false"`) because the bar and Dock stay live while one is
  open; each bar opener announces the popup and whether it is open.
  Spotlight keeps focus in its field. The shell document never scrolls, so
  no scroll lock is needed behind a sheet.

The smoke types a query to measure Spotlight's rows (`spotlight-results`)
and, on portrait, raises the host keyboard inset to check the panel ends
above it.

Unreachable gateway: a phone leaving Wi-Fi or a restarting gateway must not
look like a Home that quietly stopped updating. The host page, the one
document that talks to the gateway, counts only requests that never reached
it (a fetch `TypeError`) or a proxy's 502/503/504, retries every 3 s and
posts `home:link-status` to the shell (`home-link-status.js`). The shell
takes it only from its trusted parent (`shell-link-status.js`): the bar
shows "Reconnecting…" in a live region (a pulsing dot only on the phone bar,
static under reduced motion) and Notification Centre opens with the
explanation. The first answer clears both; a reloaded shell hears the state
again. The smoke plays the host on portrait, checks a capsule frame cannot
move it, and screenshots `notifications-reconnecting`.

### After M4 (sheets), 2026-09-24

Identical in Chromium and WebKit on the phone profiles; every shell surface
is gated at these numbers.

| Surface | Portrait targets < 44 / text < 12 | Landscape |
| --- | --- | --- |
| desktop, launcher, Spotlight (empty and with results), Control Centre, Notification Centre, Mission Control, Assistant face | 0 / 0 | 0 / 0 |
| app menu (M5, long-press on a Dock app) | 0 / 0 | 0 / 0 |
| any window | 1 / 0 (the 24 px Dock handle) | 1 / 0 |

### After M2–M3 (bar, Dock, title bar), 2026-09-23

Identical in Chromium and WebKit. The only remaining window target is the
24 px Dock handle, which is that size by design (it lives in the
home-indicator strip and must not cover the app); the launcher view toggle,
Control Centre and calendar rows are M4. Window controls became 44 px in M3.

| Surface | Portrait targets < 44 / text < 12 | Landscape | Tablet |
| --- | --- | --- | --- |
| desktop (bar + dock) | 0 / 0 | 0 / 0 | 7 / 1 |
| launcher | 1 / 0 | 1 / 0 | 8 / 1 |
| spotlight | 0 / 0 | 0 / 0 | 7 / 1 |
| control centre | 25 / 7 | 18 / 5 | 29 / 7 |
| notifications (calendar) | 1 / 40 | 1 / 3 | 8 / 41 |
| mission control | 0 / 0 | 0 / 0 | 0 / 0 |
| assistant face (shell chrome only) | 0 / 0 | 0 / 0 | 0 / 0 |
| any window | 1 / 0 (the 24 px Dock handle) | 1 / 0 | 11–12 / 1 |

### Baseline on 2026-09-23 (before any phone work)

Identical in Chromium and WebKit.

| Surface | Portrait targets < 44 / text < 12 | Landscape | Tablet |
| --- | --- | --- | --- |
| desktop (bar + dock) | 7 / 1 | 7 / 1 | 7 / 1 |
| launcher | 8 / 1 | 8 / 1 | 8 / 1 |
| spotlight | 7 / 1 | 7 / 1 | 7 / 1 |
| control centre | 32 / 8 | 24 / 6 | 29 / 7 |
| notifications (calendar) | 8 / 41 | 8 / 4 | 8 / 41 |
| mission control | 0 / 0 | 0 / 0 | 0 / 0 |
| assistant face (shell chrome only) | 0 / 0 | 0 / 0 | 0 / 0 |
| any window (bar + 12 px controls) | 10–12 / 1 | 11–12 / 1 | 11–12 / 1 |

Source: `viewport-fit=cover` absent in both index files; one bare `100vh`
(two more already had `100dvh` twins); 34 `backdrop-filter` surfaces.

Known engine noise: WebKit reports the caught cross-frame probe in
`installFrameAutoFit` (`shell-windows.js`) as a "Sandbox access violation"
page error. Chromium returns `null` silently. The shell keeps working; the
smoke records it and does not fail on it.

## Home screen (phone)

The phone's resting screen is the app grid, as on every phone OS, not the
desktop. Team review of the first phone build asked for it: the desktop
metaphor (files on the wallpaper, an Apps sheet sliding up from the Dock)
read as a desktop squeezed onto a phone.

- The grid (`#phone-home`, `shell-phone-home.js`) fills the stage between
  the bar and the Dock: 4 columns in portrait with names on up to two lines,
  6 in landscape with names on one line (which buys a second row), 60 px
  icons, names at 12 px. Until the person arranges it, it keeps the
  launcher's order (apps, then Library items); it never reshuffles as apps
  run.
- The Dock holds the Assistant and up to 4 apps. An app is in the Dock or on
  the grid, never both. Until the person arranges it the phone Dock is the
  first 4 Shelf pins; pins past the 4th stay on the grid.
- On the phone the Apps button, the desktop's files and first-run hint, and
  the Dock's Bin and running apps stand down. Library has the Desktop and
  the Bin as places; the switcher has running apps. The empty Home's menu
  offers Change Wallpaper.
- Tablet and desktop are unchanged.

The smoke's `desktop` surface on the phone profiles is the grid (0 / 0).
It checks every app sits on the grid or in the Dock exactly once, the Dock
holds at most 4 apps beside the Assistant with no Apps button, Bin or
running apps, no desktop file shows, the grid sits between the bar and the
Dock at 4 or 6 columns, and a long-press on a grid app opens its menu sheet
and launches nothing. On both phone profiles the smoke then plays the
thumb through edit mode (screenshots `home-edit`, `home-assistant`): hold a
grid app (its menu opens), move it (edit mode, the menu closes, Done shows),
push it against the right edge until the page turns and drop it there
(two pages, two dots); a tap opens nothing; a Dock app goes onto the grid
and back; Done ends the mode and keeps the arrangement; the first dot turns
back to page 1; swiping right past page 1 opens the Assistant and closing
it lands on page 1. `scripts/home-phone-home.test.mjs` covers the Dock and
grid split, the stored arrangement and every drop under `node:test`; the
server's `test_home_browser_state_drops_unknown_targets` covers the
cleaning.

### Pages

The grid is pages the thumb swipes between, one page per swipe (native
scroll snap, `shell-phone-home-pager.js`). A page holds as many rows as the
screen fits (20 apps on an iPhone 14 in portrait, 12 sideways); a page too
long for a smaller screen runs on to the next. Under the pages, the dots are
the button twin: an adjustable control (tap a dot, or the arrow keys) over a
44 px row that stays reserved on a single page, so the grid never jumps.

Left of page 1 sits the Assistant: the Agent Space that is far left of the
Space ring on every size. Swiping right from page 1 and settling there opens
the Assistant; once its room covers the floor the grid turns back to page 1
unseen, so closing the Assistant lands on the Home. The page is also a
button, for anyone who taps rather than swipes.

### Edit mode

`shell-phone-home-edit.js`, as on a phone. Hold an app until its menu opens
and move it (the menu gives way), or choose Edit Home Screen from an app's
or the empty Home's menu. The bar gives way to Done, icons jiggle, and a tap
no longer opens anything. Then any app on the grid or in the Dock can be
dragged:

- along a page (the other icons slide to their new slots);
- against a screen edge to turn the page; past the last page a new page
  opens, unless the dragged app would leave its old page empty;
- from the grid into the Dock while it has room (a full Dock takes no
  newcomer; the Assistant's tile stays put), or out of the Dock onto the
  grid. The menus offer the same as Add to Dock and Remove from Dock.

A page pushed past full hands its last app to the front of the next page.
Done, Escape or a tap on an empty spot ends the mode. Every drop is saved as
it lands. Once a hold has armed a drag the thumb moves the icon, not the
pages; in edit mode a touch on an icon never pans. Reduced motion stops the
jiggle and the slides. It is pointer events throughout, so a mouse drags in
edit mode too. On the phone the Dock rearranges only here; the desktop's
Dock drag stands down.

### The phone's own arrangement

Arranging the phone never moves the desktop. The Home layout (the per-person
state the Home saves to `/api/apps/home/state`) keeps the phone's
arrangement beside the desktop Shelf: `homeDock`, up to 4 target ids, and
`homePages`, pages of target ids. Both are absent until the first edit,
which is why an unarranged phone mirrors the Shelf and the launcher. They are
wishes, not contracts: ids this Home no longer has drop out (the server
cleans them as it cleans the Shelf's), and new installs join the last page.
No capsule API, no new authority: it is presentation state like the Shelf.

## Touch menus (M5)

Every shell menu is bound to `contextmenu`: Dock apps, Home grid apps,
launcher cards, the Bin, desktop objects and the empty desktop. Android Chrome fires it for a
held finger; iOS Safari never does, so on an iPhone those menus had no way
in. `shell-touch.js` closes the gap once for all of them:

- A touch or pen held still for 500 ms (drifting under 10 px) dispatches
  `contextmenu` at the finger. A drift, a lift or a `pointercancel` (the
  browser took the gesture to scroll) cancels it. A mouse is left alone.
- The release clicks only when no menu took the press. Holding a Dock app
  opens its menu and does not launch it; a slow tap on a plain button still
  presses it.
- The browser's own long-press `contextmenu` (Android) and ours never both
  land: whichever comes first opens the menu and the other is dropped.
- Desktop icons keep their own long-press, which also arms the touch drag.
  Their tap-to-open on touch (`shouldOpenDesktopShortcutFromClick`) stays;
  it is not a suppression of long-press.

On the phone the menu is a bottom sheet (`.context-menu-sheet`), not a
popup under the finger that hides it: full width 8 px from the edges and
the bottom, 48 px rows at 16 px, over a dimmed shell. A sheet sits away from
the icon, so an app menu starts with the app's name (the menu's accessible
name carries it at every size). A tap beside the sheet only dismisses it;
the tap does not land on whatever sat under it. Tablet and desktop keep the
popup at the pointer.

The smoke measures the menu as the `context-menu` surface and, on portrait,
long-presses a Dock app: the menu must open as the named sheet with thumb
rows, no window may open, and a tap on the bar's Control Centre button
beside it must only dismiss (screenshot `context-menu-sheet`). Playwright
cannot hold a finger down, so the probe plays the browser's touch
`pointerdown`, the hold, then the `pointerup` and click a release produces.
A touch pointer stays with the pressed tile, so the release goes to that
tile, or to the element under the finger when a redraw removed it.

The healthy fixture holds the Home event stream open. A separate
closed-stream run (portrait, both engines) ends the stream at once, so the
host keeps reconnecting and refreshing its summary, and each refresh
rebuilds the Dock. It holds the next summary reply until a finger is down on
a Dock tile, then lets it land: the tile must leave the page before the
long-press fires (the run fails when the redraw misses that window), the
held app's menu must open, and the release must launch nothing (screenshot
`phone-portrait-closed-stream/closed-stream-dock-hold`). The recovery repair
itself (a valid poll resets stream retries; a summary refresh can bypass the
retry delay) has its own owner.

## Real-device checks the smoke cannot do

These are done by a person on a phone and recorded in the PR that changes
the behaviour.

- `backdrop-filter` cost: open Control Centre and a menu sheet on a mid-range
  Android phone and an iPhone; note dropped frames in the browser's
  performance panel. If either stutters, the phone layer replaces blur with a
  flat scrim behind sheets.
- Soft keyboard: focus Spotlight and the Assistant composer; the input must
  stay visible above the keyboard on iOS Safari and Android Chrome.
- System back (Android): Back closes the top sheet or returns the window
  Home, and leaves Home from the bare desktop.
- Back on iOS: iOS has no system Back, and the shell keeps no history entry
  on WebKit (the WebKit fallback above), so the edge swipe stays Safari's own
  page back. Check the buttons instead: Close closes the window, Minimise
  returns it Home with its state, and swipe-down on the title bar opens the
  app switcher.
- Rotation with a window open keeps the window and its state.
- Long-press: holding a Dock app opens its menu sheet once on iOS Safari and
  Android Chrome, with no text selection, magnifier or link callout, and
  lifting the finger does not launch the app.
- Home pages and edit mode: a swipe turns exactly one page and never pulls
  the browser's own back gesture; holding an app then moving it drags the
  icon without scrolling the pages; an edge hold turns the page; swiping
  right from page 1 opens the Assistant. The smoke plays pointer events;
  only a real finger proves the native scroll hand-off.

## Reach and origin

A phone opens Home over HTTPS: the hosted Home or a self-hosted origin. It
cannot reach a `localhost` gateway on another machine. An installed PWA has
its own passkey world (see ARCHITECTURE.md, passkeys and origins), so the
sign-in copy explains once that installing first and then enrolling is the
smooth path. Native shells (Android WebView/GeckoView, iOS WKWebView) come
later and wrap the same Home behind the Browser/Net/Exit ABI with an explicit
host-auth adapter; there is no header bypass and no second GUI.

## Capsules on the phone stage (M6)

A capsule cannot tell a phone stage from a narrow desktop window by its own
width; only the shell knows whether traffic lights sit over the frame. So the
shell posts `elastos:shell-layout` `{ formFactor, pointer }` to every capsule
frame on load and whenever the size class changes
(`shell-capsule-layout.js`). The shared theme runtime accepts it only from the
opaque parent and only allowlisted values, and sets
`html[data-el-form-factor]` and `html[data-el-pointer]`. On phone the shared
sheet zeroes `--window-chrome-safe-top` and `--window-chrome-safe-leading`,
because the shell's title bar is above the frame, not over it. The phone
smoke checks the attribute lands in every frame that loads the shared theme,
on both engines.

A capsule sidebar becomes the shared push drawer (`elastos-drawer.js`, the
Assistant's saved-chats pattern): content first; a 44 px toggle slides the
sidebar in and pushes the view aside with a rounded edge; tapping the view,
Escape, or picking an item closes it. It is bound by markup alone
(`data-el-drawer`, `data-el-drawer-room`, `data-el-drawer-toggle`,
`data-el-drawer-close`) and vendored only to capsules that use it.
`scripts/lib/phone-drawer-assert.mjs` is the shared smoke assertion.

| Capsule | Phone layout |
| --- | --- |
| Marketplace | sidebar is the push drawer; the page title leads with its toggle; row actions keep their 28 px pill inside a 44 px target |
| Documents | the editor takes the full width; the document list is the push drawer; no Split view; Write/Read and Save in one row, the other actions in a More menu |
| Library | one header block: navigation row with the folder as title, Favorites as a sliding chip row; Search takes the navigation row; the icons/details toggle is two 44 px segments; status floats as a pill |
| System | menu button on the leading edge, on the page title's line, leads the push drawer at every phone width |
| Inbox | the filters (the phone bar has no app menu) return as a two-segment row above the list; the request list is as tall as its requests (capped at 40 dvh); the detail follows directly |
| People | 44 px buttons and 16 px profile fields |
| Archive | 44 px Open archive and New ZIP |
| Assistant | a vendor-ui target, so the room lands the size class; composer controls, the field, header buttons and session rows are 44 px, with the composer chips keeping their 32 px paint |
| Browser | the navigation row spans the stage (no traffic-light inset) with 44 px controls; a sticky status's Copy is a 44 px snackbar action |

## Capsule guidance

A first-party capsule fits the phone stage when:

- every control is at least 44 px on a coarse pointer (`--el-touch-target`
  from `capsules/_shared/elastos-ui.css`, vendored with `just vendor-ui`;
  never edit the generated per-capsule copies);
- text inputs are 16 px on a coarse pointer so iOS does not zoom the page;
- a sidebar or navigation column becomes a sheet or drawer below 640 px and
  content comes first;
- rows are at least 48 px tall;
- the capsule needs no keyboard code: the shell ends its frame above the soft
  keyboard (a framed `visualViewport` never sees the keyboard, so capsule-side
  measuring does not work);
- nothing in the capsule listens for horizontal edge swipes.

## Milestones

| Milestone | Scope |
| --- | --- |
| M0 | this charter, the phone layout smoke and its baseline |
| M1 | viewport and input truth: `viewport-fit`, overscroll, `dvh`, form-factor module, keyboard inset |
| M2 | 44 px bar with wallet/Overview in Control Centre, thumb-scrolled Dock that tucks under an open window behind a 24 px handle |
| M3 | 44 px phone title bar for every chrome mode, boot hairline, Mission Control as the app switcher (title swipe, icon captions, touch Close), stage history for system back on Chromium/Gecko (buttons-only on WebKit, recorded above) |
| M4 | launcher, Spotlight, Control Centre and Notification Centre as full sheets (the phone launcher later gave way to the Home grid) |
| M5 | touch grammar: long-press menus as bottom sheets; the phone Home as an app grid with pages and an edit mode (hold, drag, Done) |
| M6 | shell-to-capsule size class, shared tokens and push drawer; phone layouts for Marketplace, Documents, Library, System, Inbox, Browser, People, Archive and the Assistant; every measured capsule at 0 small targets on both phone profiles |
| M7 | tablet and landscape |
| M8 | PWA polish; native hosts are separate tasks |

Deferred on purpose: native shells, a service worker (the local gateway is
the offline story), haptics, and any gesture that starts on capsule content.
