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

The single source of truth for the class in JavaScript is
`shell-form-factor.js` (M1); `isPhone()` in `shell-core.js` delegates to it,
and the class is mirrored onto `document.body.dataset.formFactor` so CSS and
tests read the same fact.

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

Source: `viewport-fit=cover` absent in both index files; 3 bare `100vh`;
34 `backdrop-filter` surfaces.

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
| M2 | 44 px bar, dock that hides under an open window with a handle |
| M3 | phone title bar, instant title while loading, stage history for system back, app switcher |
| M4 | launcher, Spotlight, Control Centre and Notification Centre as full sheets |
| M5 | touch grammar: long-press menus as bottom sheets, touch drag |
| M6 | shared tokens and one PR per first-party capsule |
| M7 | tablet and landscape |
| M8 | PWA polish; native hosts are separate tasks |

Deferred on purpose: native shells, a service worker (the local gateway is
the offline story), haptics, and any gesture that starts on capsule content.
