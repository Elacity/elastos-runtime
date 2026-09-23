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
| phone | ≤ 640 px | usually coarse | one window at a time, sheets instead of popovers, 44 px chrome |
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
- `shell-form-factor.js` writes `--keyboard-inset` on `:root` from
  `visualViewport` on coarse-pointer hosts; `--stage-bottom` adds it, so an
  open window ends above the keyboard. Desktop pinch-zoom never produces an
  inset.
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
   menu right-click opens. Swipe-down on a title bar dismisses; the leading
   button does the same. No horizontal edge gestures anywhere in the shell.
5. **Truth.** Safe areas are real (`viewport-fit=cover`), the soft keyboard
   never covers a focused input (`visualViewport` → `--keyboard-inset`),
   `100dvh` not `100vh`, no rubber-band overscroll on the shell.

Capsules are graded on 1, 2 and 5 inside the phone stage; 3 and 4 are shell
responsibilities.

## Gesture surface rule

Capsules run in opaque sandboxed iframes and own every touch that lands on
them. A shell gesture may start only on shell chrome: the title bar, the dock
handle, a sheet handle, the desktop. The shell never listens for a swipe
that begins over capsule content, and never uses a horizontal edge gesture,
because Android back and iOS back both live there.

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

For the desktop, launcher, Spotlight, Control Centre, Notification Centre,
Mission Control, the Assistant face and one window per first-party app it
records targets under 44 px, text under 12 px and horizontal overflow, plus
the source truths that need no browser (`viewport-fit=cover` in both
`index.html` files, bare `100vh` count, `backdrop-filter` count). It writes
`report.json` and a screenshot per surface to `HOME_PHONE_SMOKE_OUT` (default
a temp directory).

The gate is a ratchet. `BASELINE` in the script pins today's numbers per
profile and surface; a run may not exceed them. A PR that improves a surface
lowers its row in the same commit. Nothing raises a row.

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
  Escape, or picking a window tucks it again. Mission Control, the launcher
  face and keyboard focus inside the Dock always show it. The desktop
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

## Real-device checks the smoke cannot do

These are done by a person on a phone and recorded in the PR that changes
the behaviour.

- `backdrop-filter` cost: open Control Centre and the launcher on a mid-range
  Android phone and an iPhone; note dropped frames in the browser's
  performance panel. If either stutters, the phone layer replaces blur with a
  flat scrim behind sheets.
- Soft keyboard: focus Spotlight and the Assistant composer; the input must
  stay visible above the keyboard on iOS Safari and Android Chrome.
- System back: Android back and the iOS edge swipe must close the top sheet
  or return Home, and must leave Home from the bare desktop.
- Rotation with a window open keeps the window and its state.

## Reach and origin

A phone opens Home over HTTPS: the hosted Home or a self-hosted origin. It
cannot reach a `localhost` gateway on another machine. An installed PWA has
its own passkey world (see ARCHITECTURE.md, passkeys and origins), so the
sign-in copy explains once that installing first and then enrolling is the
smooth path. Native shells (Android WebView/GeckoView, iOS WKWebView) come
later and wrap the same Home behind the Browser/Net/Exit ABI with an explicit
host-auth adapter; there is no header bypass and no second GUI.

## Capsule guidance

A first-party capsule fits the phone stage when:

- every control is at least 44 px on a coarse pointer (`--tap-min` from
  `capsules/_shared/elastos-ui.css`, vendored with `just vendor-ui`; never
  edit the generated per-capsule copies);
- text inputs are 16 px on a coarse pointer so iOS does not zoom the page;
- a sidebar or navigation column becomes a sheet or drawer below 640 px and
  content comes first;
- rows are at least 48 px tall;
- the capsule handles its own `visualViewport` for composers (capsules stay
  opaque; the shell does not reach in);
- nothing in the capsule listens for horizontal edge swipes.

## Milestones

| Milestone | Scope |
| --- | --- |
| M0 | this charter, the phone layout smoke and its baseline |
| M1 | viewport and input truth: `viewport-fit`, overscroll, `dvh`, form-factor module, keyboard inset |
| M2 | 44 px bar with wallet/Overview in Control Centre, thumb-scrolled Dock that tucks under an open window behind a 24 px handle |
| M3 | phone title bar, instant title while loading, stage history for system back, app switcher |
| M4 | launcher, Spotlight, Control Centre and Notification Centre as full sheets |
| M5 | touch grammar: long-press menus as bottom sheets, touch drag |
| M6 | shared tokens and one PR per first-party capsule |
| M7 | tablet and landscape |
| M8 | PWA polish; native hosts are separate tasks |

Deferred on purpose: native shells, a service worker (the local gateway is
the offline story), haptics, and any gesture that starts on capsule content.
