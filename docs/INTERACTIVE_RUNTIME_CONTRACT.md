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
  - `elastos pc2`
  - `PC2 -> Chat`
- Explicit operator lane:
  - `elastos serve`
  - operator-only commands layered on top of that runtime
- `elastos room open` is not a second host. It is the explicit helper that asks the live operator runtime to expose the room gateway.
- Today `elastos` does not attach to an already-running operator runtime in the same home. If you switch lanes, stop the current host or use a different home.

## First-Class Paths

These are the blessed public interactive paths:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos` | managed `pc2` runtime | first-class |
| `elastos pc2` | managed `pc2` runtime | first-class |
| `PC2 -> Chat` | same managed `pc2` runtime | first-class |

These are supported shortcuts, but they are not the primary product story:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos chat` | reuse healthy managed `pc2` runtime first; otherwise managed `chat` runtime | secondary supported shortcut |
| `elastos capsule <name> --lifecycle interactive --interactive` | reuse active runtime when compatible; otherwise managed `pc2` runtime | secondary packaged surface path |

These are explicit operator or developer surfaces, not part of the boring front door:

| Path | Runtime owner | Status |
|---|---|---|
| `elastos agent` | operator runtime (`elastos serve`) | operator-only |
| `elastos run` (WASM / microVM) | operator runtime (`elastos serve`) | developer / operator-only |
| `elastos capsule ...` (non-interactive) | operator runtime (`elastos serve`) | operator-only |

## Runtime Ownership

### 1. PC2 owns the main user story

`elastos` and `elastos pc2` auto-start or reuse the managed `pc2` runtime. That runtime is the canonical owner for the dashboard, Home, and launched first-class actions.

It is a different lane from explicit `elastos serve`. The current public line does not pretend those two live hosts are interchangeable.

### 2. Native chat is the canonical chat surface

The canonical chat story is native chat:

- launched from `PC2 -> Chat`, or
- entered directly through `elastos chat` as a shortcut

`elastos chat` is not a different product. It is a shortcut to the same native chat surface. It first tries to reuse a healthy managed `pc2` runtime. If no healthy managed `pc2` runtime exists, it starts or reuses the managed `chat` runtime.

### 3. Packaged interactive capsules are not automatically first-class

`elastos capsule <name> --lifecycle interactive --interactive` is a real supported mechanism, but it is not the same as the public front door.

Current rule:

- if a packaged surface is explicitly shipped, surfaced in PC2, and proven on the installed path, it may be treated as a user-facing PC2 action
- otherwise it remains a secondary or developer-oriented packaged launch path

This keeps the product contract narrow and prevents proof-only surfaces from masquerading as first-class UX.

## Input And Exit Semantics

### PC2 Home

| Input | Result |
|---|---|
| `Enter` | launch the selected action |
| `q`, `quit`, `/q`, `/quit` | leave PC2 and return to the invoking terminal |
| `Esc` | no stable global meaning in PC2 Home; do not document it as a home/quit contract |

PC2 owns the terminal while Home is visible. Launched actions temporarily take over the same terminal and return control to PC2 when they exit cleanly.

### Native Chat launched from PC2

| Input | Result |
|---|---|
| `Esc` | leave chat and return to the same PC2 session |
| `/home` | leave chat and return to the same PC2 session |
| `/quit`, `/q` | leave chat; the parent PC2 session regains control |

Effective result: all three paths get the user back to the same PC2 session. The semantic distinction is:

- `Esc` and `/home` mean "return home"
- `/quit` means "leave chat and hand control back to PC2"

### Standalone native chat: `elastos chat`

| Input | Result |
|---|---|
| `Esc` | open or return to PC2 |
| `/home` | open or return to PC2 |
| `/quit`, `/q` | exit chat and return to the invoking terminal |

This is the only standalone shortcut that carries a blessed home-return contract today.

### Direct packaged chat-family surfaces

The `chat` and `chat-wasm` capsules share the same command grammar:

- `Esc` and `/home` request a home exit
- `/quit`, `/q`, `/exit` request a chat exit

But the caller decides what "home" means:

- when launched from PC2, the user ends up back at PC2
- when launched directly from the terminal with `elastos capsule ... --interactive`, both `/home` and `/quit` exit back to the invoking terminal

That is why direct packaged chat-family launch remains secondary today, even though the capsule-level commands exist.

### Operator surfaces

`elastos agent`, non-interactive `elastos capsule ...`, and explicit `elastos run ...` do not share the PC2 home contract.

They are explicit operator or developer surfaces:

- no implicit return-home promise
- no `Esc` / `/home` public contract
- fail fast if `elastos serve` is not already running

## Proof Matrix To Keep

The interactive contract is not frozen unless these remain green:

- native/native chat on same host
- native/native chat cross-host
- native `elastos chat` after prior PC2 use
- `elastos -> PC2 -> Chat -> /home`
- packaged chat-family launch from PC2, if the surface is advertised in PC2
- packaged chat-family direct launch, if the surface is advertised as supported outside PC2
- native/WASM and native/microVM interop only if those paths are still claimed as active product surfaces

If a path is not proven, it should not keep first-class wording in docs or UI.
