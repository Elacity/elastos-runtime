# ElastOS design system

The linked source files own their token values. The repository has several
first-party palettes rather than one global token package.

## Current token families

- The [Home shell host](../capsules/home/browser/style.css) and
  [Home GUI](../capsules/home-gui/browser/style.css) define the same core roles
  with dark glass, neutral text, and ElastOS orange. Either surface can render
  independently.
- [Chat Room](../capsules/chat-room/browser/style.css) and the
  [Documents page](../capsules/documents/browser/index.html)
  share the lavender content palette.
- [Inbox](../capsules/inbox/browser/index.html),
  [Library](../capsules/library/browser/library.css), and
  [System](../capsules/system/browser/style.css) use related neutral utility
  styling. Each has its own values and token names.
- Other capsules keep local palettes when their content, contrast, or theme
  model requires one. A local palette is not a shared contract.

Within a family, tokens describe roles rather than colors: background, surface,
border, primary and muted text, action or selection, brand, and feedback.
`soft` and `strong` variants are relative to their family. ElastOS orange marks
product identity; it is not the default color for every action.

## Implementation baseline

First-party browser surfaces start with
plain ES modules or native Web Components. A framework or compiler such as
Svelte stays capsule-local and optional. It must not become a dependency of
Runtime, ESP, or shared contract packages.

Framework choice does not change authority. Browser code still uses the
capsule's declared Runtime interfaces and cannot gain privileges from its DOM,
router, or build system.

## Interaction

Every visible action must have the same contract for humans and agents.
[Principle 7](../PRINCIPLES.md#7-humans-and-agents-share-one-authority-model)
owns that authority rule. Visual controls must still have readable names,
keyboard focus, sufficient contrast, and in-surface confirmation for destructive
actions.

## Verification

The [Home entropy check](../scripts/home-entropy-check.mjs) freezes selected
token values for Home, Chat Room, Documents, Inbox, Library, and System. It also
checks accessible names and stale copy across selected active surfaces. Its
coverage is limited to those files and does not define a repository-wide
palette.
