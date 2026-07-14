# Home (`home`)

This directory owns the runtime-owned browser-hosted adapter for the Home
front-door host. It is not the selectable GUI shell; that identity is
`home-gui`.

Current truth:

1. `home` is the internal capsule ID for Home.
   - the capsule manifest entrypoint is `home.wasm`
   - the capsule role is `app` because this is the front-door host, not a
     selectable shell
   - browser assets live under `browser/`
   - the runtime serves that surface at `/apps/home/`
   - legacy saved active-shell state may use `home` only as a migration value
     that repairs to `home-gui`; new active-shell writes must not use `home`

2. Home mounts exactly one active shell through runtime-owned summary and launch routes.
   - `home-gui` owns desktop, launcher, taskbar, and app windows as trusted
     host-loaded GUI shell code
   - `home-cli` owns the terminal viewport while selected through the
     Runtime-owned PTY path
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
- `home-gui` is the current exception to iframe mounting: it is selected by
  Runtime state, then lazy-loaded by the trusted Home host facade
- recovery is host-owned and must not depend on `home-gui` being mounted
- `home` must not contain desktop/taskbar/window/launcher templates or behavior
- GUI sessions, desktop shortcuts, taskbar state, and app-window placement belong to
  `home-gui`

See:
- [../../TASKS.md](../../TASKS.md)
- [../../state.md](../../state.md)
- [../../ROADMAP.md](../../ROADMAP.md)
