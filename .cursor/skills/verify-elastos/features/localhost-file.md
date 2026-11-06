# Localhost file

A shell operator saves a text file under `localhost://Public` and reads the same bytes back. A client session without a capability token cannot write that file.

## Sub-features

- `file-shell-save` writes `Public/verify-note.txt` with a shell attach token.
- `file-shell-read` reads those bytes back through the API and from disk.
- `file-client-refused` rejects the same write from a client attach token.
- `file-anonymous-refused` rejects the same write with no Authorization header.

## How to get to it (user POV)

- Start the operator runtime with `elastos serve`.
- Attach with the local secret and scope `shell`, then `PUT /api/localhost/Public/verify-note.txt`.
- Attach with scope `client` and send the same PUT. The runtime refuses it.
- Send the PUT with no Authorization header. The runtime refuses it.
- `GET /api/localhost/Public/verify-note.txt` with the shell token.

## Driving it with verify-elastos

Preconditions:

- `doctor.sh` reports the run pid, port, and version.
- The path `Public/verify-note.txt` is absent in that run's data home.

- **Refuse the client.** Attach with scope `client`. Run `.cursor/skills/verify-elastos/scripts/drive-localhost-file.sh "$ENV"`. The transcript line `client_put=403` is present.
- **Refuse an anonymous write.** The same command records `unauth_put=401`.
- **Save as shell.** The same command attaches with scope `shell` and records `shell_put=200`.
- **Read it back.** The same command records `shell_get=200` and `read_body=Release checklist`.
- **Confirm the file.** The transcript records `disk_matches=True` and `disk_path` ends in `Public/verify-note.txt`.

## Gotchas

- A client token is not a shell token. Scope `client` returns session type `capsule`.
- `Public/verify-note.txt` must be a rooted localhost path. A bare `verify-note.txt` is rejected before the write.
- `Users/...` is a principal root. This recipe does not use it.
- A 200 from the PUT is not enough. The GET body and the file under the data home must both equal `Release checklist`.
- Do not copy the attach secret into the evidence file.
