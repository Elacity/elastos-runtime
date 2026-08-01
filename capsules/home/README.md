# Home (`home`)

`home` owns sign-in, recovery, root-shell lifecycle, and validated child
messages. It is not a selectable shell and does not implement the desktop or
terminal. Those projections belong to `home-gui` and `home-cli`.

The manifest declares `role=app` and `execution=web-projection`. Both
`runtime_abi` and `bus_contract` are `elastos.runtime-projection/v1`. Its entry
point is `browser/index.html`.

Runtime serves the projection at `/apps/home/`. Browser assets live under
`browser/`.

The [Home shell host contract](../../docs/HOME_SHELL_HOST_CONTRACT.md) owns the
authority, isolation, lifecycle, and message rules. See
[state.md](../../state.md) for verified implementation status and
[TASKS.md](../../TASKS.md) for open work.
