#!/usr/bin/env python3
"""Exhaustive authority matrix for localhost writes on one running runtime."""

import json
import pathlib
import sys
import urllib.error
import urllib.request


def load_env(path: pathlib.Path) -> dict[str, str]:
    env = {}
    for line in path.read_text().splitlines():
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        env[key] = value
    return env


def request(base: str, method: str, path: str, body=None, headers=None):
    req = urllib.request.Request(
        base + path, data=body, headers=headers or {}, method=method
    )
    try:
        with urllib.request.urlopen(req, timeout=5) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as err:
        return err.code, err.read()


def expect(rows, name, status, body, wanted_status, body_has=None, body_equals=None):
    ok = status == wanted_status
    if body_has is not None and body_has not in body:
        ok = False
    if body_equals is not None and body != body_equals:
        ok = False
    rows.append(
        {
            "name": name,
            "status": status,
            "wanted": wanted_status,
            "ok": ok,
            "body": body[:180],
        }
    )
    return ok


def main() -> int:
    env = load_env(pathlib.Path(sys.argv[1]))
    out = pathlib.Path(sys.argv[2])
    base = f"http://127.0.0.1:{env['PORT']}"
    data = pathlib.Path(env["DATA"])
    secret = json.loads((data / "runtime-coords.json").read_text())["attach_secret"]
    payload = b"Release checklist"
    path = "/api/localhost/Public/verify-note.txt"
    rows = []
    failed = False

    status, raw = request(base, "GET", "/api/health")
    failed |= not expect(rows, "health", status, raw.decode(), 200, body_has='"status":"ok"')

    secrets = {
        "correct": secret,
        "empty": "",
        "wrong": "not-the-secret",
        "suffix": secret + "x",
    }
    tokens = {}
    for label, presented in secrets.items():
        for scope in ("shell", "client", "operator"):
            status, raw = request(
                base,
                "POST",
                "/api/auth/attach",
                json.dumps({"secret": presented, "scope": scope}).encode(),
                {"Content-Type": "application/json"},
            )
            text = raw.decode(errors="replace")
            name = f"attach-{label}-{scope}"
            if label == "correct":
                wanted_type = "shell" if scope == "shell" else "capsule"
                ok = status == 200 and f'"session_type":"{wanted_type}"' in text.replace(" ", "")
                # session_type may be serialized with a space after the colon.
                if not ok and status == 200:
                    body = json.loads(text)
                    ok = body.get("session_type") == wanted_type
                    if ok:
                        tokens[scope] = body["token"]
                elif ok:
                    tokens[scope] = json.loads(text)["token"]
                rows.append(
                    {
                        "name": name,
                        "status": status,
                        "wanted": 200,
                        "ok": ok,
                        "body": f"session_type check {wanted_type}",
                    }
                )
                failed |= not ok
            else:
                failed |= not expect(
                    rows, name, status, text, 403, body_has="invalid attach secret"
                )

    missing = [scope for scope in ("shell", "client", "operator") if scope not in tokens]
    if missing:
        rows.append(
            {
                "name": "attach-tokens",
                "status": 0,
                "wanted": 200,
                "ok": False,
                "body": "missing tokens for " + ",".join(missing),
            }
        )
        failed = True
        out.parent.mkdir(parents=True, exist_ok=True)
        text = "\n".join(
            f"{'ok' if row['ok'] else 'FAIL'} {row['name']} status={row['status']} wanted={row['wanted']} {row['body']}"
            for row in rows
        ) + "\n"
        out.write_text(text)
        sys.stdout.write(text)
        return 1

    shell = {"Authorization": f"Bearer {tokens['shell']}"}
    client = {"Authorization": f"Bearer {tokens['client']}"}
    other = {"Authorization": f"Bearer {tokens['operator']}"}
    bad = {"Authorization": "Bearer not-a-session"}
    empty_bearer = {"Authorization": "Bearer "}
    client_cap = {
        "Authorization": f"Bearer {tokens['client']}",
        "X-Capability-Token": "not-a-capability",
    }

    cases = [
        ("write-no-auth", {}, 401, "Missing Authorization header"),
        ("write-empty-bearer", empty_bearer, 401, None),
        ("write-bad-bearer", bad, 401, "Invalid or expired session token"),
        ("write-client", client, 403, "missing X-Capability-Token"),
        ("write-operator", other, 403, "missing X-Capability-Token"),
        ("write-client-bad-capability", client_cap, 403, "invalid token"),
        ("write-shell", shell, 200, '"size"'),
    ]
    for name, headers, wanted, needle in cases:
        status, raw = request(base, "PUT", path, payload, headers)
        text = raw.decode(errors="replace")
        failed |= not expect(rows, name, status, text, wanted, body_has=needle)

    status, raw = request(base, "GET", path, None, shell)
    failed |= not expect(
        rows, "read-shell", status, raw.decode(errors="replace"), 200, body_equals="Release checklist"
    )
    status, raw = request(base, "GET", path, None, client)
    failed |= not expect(
        rows,
        "read-client",
        status,
        raw.decode(errors="replace"),
        403,
        body_has="missing X-Capability-Token",
    )

    disk = data / "Public" / "verify-note.txt"
    disk_ok = disk.is_file() and disk.read_bytes() == payload
    rows.append(
        {
            "name": "disk-public-file",
            "status": 200 if disk_ok else 0,
            "wanted": 200,
            "ok": disk_ok,
            "body": str(disk),
        }
    )
    failed |= not disk_ok

    out.parent.mkdir(parents=True, exist_ok=True)
    lines = []
    for row in rows:
        mark = "ok" if row["ok"] else "FAIL"
        lines.append(f"{mark} {row['name']} status={row['status']} wanted={row['wanted']} {row['body']}")
    text = "\n".join(lines) + "\n"
    out.write_text(text)
    sys.stdout.write(text)
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
