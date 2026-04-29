# Interactive Runtime Contract

This document freezes the interactive command model for the current public line.

The goal is simple:

- one blessed front door
- one explicit runtime owner per path
- one honest meaning for `Esc`, `/home`, and `/quit`
- no pretending that every interactive surface is equally product-ready

See [COMMAND_MATRIX.md](COMMAND_MATRIX.md) for the full command/runtime table. This document narrows that matrix to the interactive surfaces users actually feel.

## Lane Selection And Host Ownership

One data home may have one live host owner at a time.

- Managed dashboard lane:
  - `elastos`
  - `elastos home`
  - `Home -> System/Documents/Library/Inbox`
- Explicit operator lane:
  - `elastos serve`
  - operator-only commands layered on top of that runtime
- `elastos room open` is not a second host. It is the explicit helper that asks the live operator runtime to expose the room gateway.
- Today `elastos` does not attach to an already-running operator runtime in the same home. If you switch lanes, stop the current host or use a different home.

## First-Class Paths

These are the blessed public interactive paths:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos` | managed Home runtime | first-class |
| `elastos home` | managed Home runtime | first-class |
| `Home -> System/Documents/Library/Inbox` | same managed Home runtime | first-class |

These are supported shortcuts, but they are not the primary product story:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos chat` | reuse healthy managed Home runtime first; otherwise managed `chat` runtime | secondary supported shortcut |
| `elastos capsule <name> --lifecycle interactive --interactive` | reuse active runtime when compatible; otherwise managed Home runtime | secondary packaged surface path |

These are explicit operator or developer surfaces, not part of the boring front door:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos agent` | operator runtime (`elastos serve`) | operator-only |
| `elastos run` (WASM / microVM) | operator runtime (`elastos serve`) | developer / operator-only |
| `elastos capsule ...` (non-interactive) | operator runtime (`elastos serve`) | operator-only |

## Runtime Ownership

### 1. Home owns the main user story

`elastos` and `elastos home` auto-start or reuse the managed Home runtime. That runtime is the current host lane for Home and launched first-class actions.

It is a different lane from explicit `elastos serve`. The current public line does not pretend those two live hosts are interchangeable.

### 2. Native chat is a supported shortcut

The direct native chat story is:

- entered through `elastos chat` as a shortcut

`elastos chat` first tries to reuse a healthy managed Home runtime. If no healthy managed Home runtime exists, it starts or reuses the managed `chat` runtime.

The browser `Chat Room` surface is separate from the native chat shortcut. It is shipped with the demo profile and hosted through the explicit operator lane when exposed outside Home.

### 3. Packaged interactive capsules are not automatically first-class

`elastos capsule <name> --lifecycle interactive --interactive` is a real supported mechanism, but it is not the same as the public front door.

Current rule:

- if a packaged surface is explicitly shipped, surfaced in Home, and proven on the installed path, it may be treated as a user-facing Home action
- otherwise it remains a secondary or developer-oriented packaged launch path

This keeps the product contract narrow and prevents proof-only surfaces from masquerading as first-class UX.

## Input And Exit Semantics

### Home

| Input | Result |
|---|---|
| `Enter` | launch the selected action |
| `q`, `quit`, `/q`, `/quit` | leave Home and return to the invoking terminal |
| `Esc` | no stable global meaning in Home; do not document it as a home/quit contract |

Home owns the terminal while visible. Launched actions temporarily take over the same terminal and return control to Home when they exit cleanly.

### Standalone native chat: `elastos chat`

| Input | Result |
|---|---|
| `Esc` | open or return Home |
| `/home` | open or return Home |
| `/quit`, `/q` | exit chat and return to the invoking terminal |

This is the only standalone shortcut that carries a blessed home-return contract today.

### Direct packaged chat-family surfaces

The `chat` and `chat-wasm` capsules share the same command grammar:

- `Esc` and `/home` request a home exit
- `/quit`, `/q`, `/exit` request a chat exit

But the caller decides what "home" means:

- when launched from Home, the user ends up back at Home
- when launched directly from the terminal with `elastos capsule ... --interactive`, both `/home` and `/quit` exit back to the invoking terminal

That is why direct packaged chat-family launch remains secondary today, even though the capsule-level commands exist.

### Operator surfaces

`elastos agent`, non-interactive `elastos capsule ...`, and explicit `elastos run ...` do not share the Home contract.

They are explicit operator or developer surfaces:

- no implicit return-home promise
- no `Esc` / `/home` public contract
- fail fast if `elastos serve` is not already running

## Proof Matrix To Keep

The interactive contract is not frozen unless these remain green:

- native/native chat on same host
- native/native chat cross-host
- native `elastos chat` after prior Home use
- `elastos -> Home -> System/Documents/Library/Inbox -> Home`
- packaged chat-family launch from Home, if the surface is advertised in Home
- packaged chat-family direct launch, if the surface is advertised as supported outside Home
- native/WASM and native/microVM interop only if those paths are still claimed as active product surfaces

If a path is not proven, it should not keep first-class wording in docs or UI.
