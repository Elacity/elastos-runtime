# Home (`home`)

This directory owns the runtime-owned browser-hosted adapter for the Home capsule.

Current truth:

1. `home` is the internal capsule ID for Home.
   - the capsule manifest entrypoint is `home.wasm`
   - browser assets live under `browser/`
   - the runtime serves that surface at `/apps/home/`

2. Home launches first-party apps through runtime-owned summary and launch routes.
   - Home renders only targets that the runtime reports as available
   - browser-capable targets attach inside Home windows through same-origin iframes
   - later first-party apps should add their own capsule source and package metadata in the same review slice

3. Home is the shipped browser front door.
   - the browser route is `/apps/home/`
   - the CLI entrypoint is `elastos home` and the default `elastos` command

What to prove next:
- keep installed-path `Home -> runtime-reported target -> Home` proof boring on target machines
- decide the first non-browser attach contract after the browser launch loop is stable
- keep Home as an orchestrator, not a place where app/provider policy leaks into UI code

Interaction contract for Home:
- every meaningful action must work through browser-level interaction, not only internal DOM invocation
- passive facts like runtime status or local identity must not masquerade as buttons
- visible Home state must be inspectable from the DOM:
  - launcher open/closed
  - desktop selection
  - taskbar open/active state
  - window hidden/active/maximized state
- refresh should restore the Home-owned window session:
  - open targets
  - hidden/visible state
  - window placement and special state
- desktop shortcuts use desktop semantics:
  - single click selects
  - double click or `Enter` opens
- Home should prefer simple, standards-aligned hit targets over layout tricks that make automation brittle

See:
- [../../TASKS.md](../../TASKS.md)
- [../../state.md](../../state.md)
- [../../ROADMAP.md](../../ROADMAP.md)
