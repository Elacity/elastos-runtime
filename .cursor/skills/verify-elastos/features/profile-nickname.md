# Profile nickname

A person sets the local profile nickname and reads it back from a second CLI command. The nickname is stored for that data home only.

## Sub-features

- `nick-unset` shows a new profile with no nickname.
- `nick-set` stores a nickname.
- `nick-get` prints the stored nickname.
- `nick-show` shows the same nickname on the profile.

## How to get to it (user POV)

- Run `elastos identity show` against an empty data home.
- Run `elastos identity nickname set "Release checklist"`.
- Run `elastos identity nickname get`.
- Run `elastos identity show` again.

## Driving it with verify-elastos

Preconditions:

- `$BIN` and `$XDG_DATA_HOME` come from a `run.env`, or from a second empty directory when a runtime is already serving that home.
- Do not run these commands against a data home that `elastos serve` is hosting.

- **Empty profile.** Run `XDG_DATA_HOME="$XDG_DATA_HOME" "$BIN" identity show`. Stdout contains `Nickname:  (not set)` and the exit code is 0.
- **Set nickname.** Run `XDG_DATA_HOME="$XDG_DATA_HOME" "$BIN" identity nickname set "Release checklist"`. Stdout contains `Nickname set to 'Release checklist'.`
- **Read it back.** Run `XDG_DATA_HOME="$XDG_DATA_HOME" "$BIN" identity nickname get`. Stdout is `Release checklist`.
- **Profile view.** Run `XDG_DATA_HOME="$XDG_DATA_HOME" "$BIN" identity show`. Stdout contains `Nickname:  Release checklist`.

## Gotchas

- An omitted nickname value in a non-interactive shell exits with an error. Pass the value on the command line.
- The stored nickname is encrypted. Proof is the second CLI read, not a raw file compare.
- A nickname longer than 32 characters is rejected. `Release checklist` is within that limit.
- Unset `XDG_DATA_HOME` writes the operator's own Home. Do not do that from this recipe.
