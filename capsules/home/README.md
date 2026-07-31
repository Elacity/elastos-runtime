# Home (`home`)

This directory owns the runtime-owned browser-hosted adapter for the Home
front-door host. It is not the selectable GUI shell; that identity is
`home-gui`.

Current truth:

1. `home` is the internal capsule ID for Home.
   - the capsule manifest entrypoint is `browser/index.html`
   - the capsule declares `elastos.runtime-projection/v1`, not a WASI receipt
   - the capsule role is `app` because this is the front-door host, not a
     selectable shell
   - browser assets live under `browser/`
   - the runtime serves that surface at `/apps/home/`
   - active-shell writes use only `home-gui` or `home-cli`; `home` is never a
     selectable shell

2. Home mounts exactly one active shell through runtime-owned summary and launch routes.
   - `home-gui` owns desktop, launcher, taskbar, and app windows in its opaque
     sandboxed frame
   - `home-cli` owns the terminal viewport and Runtime-owned PTY path in its
     opaque sandboxed frame
   - both shells share the same Runtime facts, lifecycle, launch validation,
     sign-out, and explicit shell-switch authority
   - child shell/app intents must carry the Home launch token and pass host routing policy

3. Home is the shipped browser front door.
   - the browser route is `/apps/home/`
   - the CLI entrypoint is `elastos home` and the default `elastos` command

What to prove next:
- keep installed-path `Home -> runtime-reported target -> Home` proof boring on target machines
- decide the first non-browser attach contract after the browser launch loop is stable
- keep Home as an orchestrator, not a place where app/provider policy leaks into UI code

Interaction contract for the Home host:
- unlock and sign-out stay in the host plus Runtime auth endpoints
- active-shell selection is resolved from Runtime state, not local UI state
- shell mounting uses a host-owned root iframe plus launch-token validation
- the top-level host retains one cryptographically random, bounded browser
  profile correlation and hands it only to the exact active `home-gui` frame
  after its opaque-origin, source, target, and launch token are accepted
- both shells and ordinary app frames use opaque browser origins on the same
  hostname; only the Home host
  holds the explicit authority used to mint their launch tokens
- recovery is host-owned and must not depend on `home-gui` being mounted
- `home` must not contain desktop/taskbar/window/launcher templates or behavior
- GUI sessions, desktop shortcuts, taskbar state, and app-window placement belong to
  `home-gui`
- the browser profile correlation grants no authority and never replaces the
  encrypted principal-scoped Runtime Home state

See:
- [../../TASKS.md](../../TASKS.md)
- [../../state.md](../../state.md)
- [../../ROADMAP.md](../../ROADMAP.md)
