# ElastOS Design System

This is the active visual contract for Home and the first-party browser capsules.
It is intentionally small: one brand layer, one capsule layer, and no per-app
color personality unless the app has a functional reason.

## Color Contract

Home sits on the user wallpaper and ElastOS mark. It uses a dark glass layer with
the ElastOS orange as brand emphasis:

| Token | Value | Use |
|------|-------|-----|
| `--brand` | `#f6921a` | ElastOS logo-adjacent emphasis, badges, selected chrome |
| `--brand-strong` | `#ffb457` | hover/focus brand emphasis |
| `--bg` | `#050608` | Home base behind wallpaper |
| `--text` | `#f5f7fa` | Home foreground text |

First-party capsules use a light, document-like layer over the same product
world. The palette is shared by Chat Room, Documents, Inbox, Library, System,
and GBA:

| Token | Value | Use |
|------|-------|-----|
| `--bg` | `#edf1fb` | capsule background |
| `--bg-strong` | `#e3e9fb` | stronger capsule wash |
| `--panel` | `rgba(255, 255, 255, 0.9)` | primary glass cards |
| `--panel-strong` | `#ffffff` | solid cards and inputs |
| `--panel-soft` | `#eef2ff` | secondary controls |
| `--line` | `rgba(83, 103, 164, 0.14)` | normal borders |
| `--line-strong` | `rgba(83, 103, 164, 0.22)` | focusable borders |
| `--ink` | `#1d2438` | foreground text |
| `--muted` | `#66708a` | secondary text |
| `--accent` | `#5f76d8` | primary action and selected state |
| `--accent-soft` | `#e8edff` | selected/soft action background |
| `--accent-deep` | `#3c53a7` | strong accent text |
| `--danger` | `#b14c5a` | destructive actions |
| `--brand` | `#f6921a` | ElastOS brand accent, used sparingly |
| `--brand-soft` | `#fff1dc` | soft brand background |

The capsule accent is blue-lavender because it holds contrast against the pale
wallpaper and lets the orange logo remain the brand anchor instead of competing
with every button.

## Interaction Contract

Every visible action must have the same contract for humans and agents:

- A human can use it with pointer, keyboard, and readable labels.
- An agent can use the same capability-scoped API, action id, or Home message.
- DOM visibility or route shape is never authority.
- Destructive actions use in-surface confirmation and provider/runtime calls,
  not browser alerts or hidden privileged paths.
- First-party surfaces expose state in simple product nouns before raw paths,
  CIDs, or provider details.

## Drift Checks

`scripts/home-entropy-check.mjs` enforces the active token set, stale-copy
removal, and the basic human/agent interaction contract for first-party browser
surfaces. Update this document and the check together when the design language
intentionally changes.
