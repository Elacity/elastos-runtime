# Operator attach

An operator confirms the runtime is up, exchanges the local attach secret for a session, and sees a wrong secret refused.

## Sub-features

- `attach-health` reads `GET /api/health`.
- `attach-ok` posts the secret from `runtime-coords.json` with scope `client`.
- `attach-rejected` posts a different secret.

## How to get to it (user POV)

- Run `elastos serve` and open `http://127.0.0.1:<port>/api/health`.
- Read `attach_secret` from the run's `runtime-coords.json`.
- `POST /api/auth/attach` with JSON `{"secret":"<secret>","scope":"client"}`.
- Repeat the POST with secret `not-the-secret`.

## Driving it with verify-elastos

Preconditions:

- `doctor.sh` reports the run pid and port.
- `runtime-coords.json` exists in the run's data home and is mode `0600`.

- **Health.** Run `curl -fsS "http://127.0.0.1:$PORT/api/health"`. The body contains `"status":"ok"`.
- **Accept the secret.** POST the secret and scope `client`. The status is 200 and `session_type` is `capsule`.
- **Reject a wrong secret.** POST `not-the-secret`. The status is 403 and the body contains `invalid attach secret`.
- **Session.** `GET /api/session` with `Authorization: Bearer <token>` from the accepted attach. The status is 200 and `session_type` is `capsule`.

## Gotchas

- The secret is local to the run's data home. A secret from another home is a wrong secret.
- `GET /api/session` without a bearer token returns 401. That is not a failed attach.
- Record status codes and `session_type`. Do not record the secret or the bearer token.
