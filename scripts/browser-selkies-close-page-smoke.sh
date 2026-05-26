#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d /tmp/elastos-browser-selkies-close-page-smoke-XXXXXX)"
server_pid=""
cleanup() {
  if [[ -n "$server_pid" ]]; then
    kill "$server_pid" >/dev/null 2>&1 || true
  fi
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

socket_path="$tmp_dir/selkies-control.sock"
page_id="page:selkies-smoke"

node "$repo_root/scripts/browser-selkies-close-page.mjs" \
  --control-socket "$socket_path" \
  --page-id "$page_id" \
  >"$tmp_dir/dry-run.json"

node -e '
  const fs = require("node:fs");
  const dry = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  function fail(message) {
    console.error(message);
    process.exit(1);
  }
  if (dry.schema !== "elastos.browser.selkies-close-page/v1" || dry.ok !== true || dry.dry_run !== true) {
    fail("dry run must emit a safe close-page receipt");
  }
  if (!dry.confirm_command.includes("--confirm-close") || !dry.confirm_command.includes("page:selkies-smoke")) {
    fail("dry run must print the explicit confirmed command");
  }
' "$tmp_dir/dry-run.json"

set +e
node "$repo_root/scripts/browser-selkies-close-page.mjs" \
  --control-socket "$socket_path" \
  --page-id "$page_id" \
  --confirm-close \
  >"$tmp_dir/missing.out" \
  2>"$tmp_dir/missing.err"
missing_status=$?
set -e

if [[ "$missing_status" -eq 0 ]]; then
  echo "confirmed close must fail when the control socket is unavailable" >&2
  exit 1
fi
if ! grep -q "control socket is not available" "$tmp_dir/missing.err"; then
  echo "missing-socket close did not explain the fail-closed reason" >&2
  cat "$tmp_dir/missing.err" >&2
  exit 1
fi

node - "$socket_path" "$page_id" <<'NODE' &
const http = require("node:http");
const socketPath = process.argv[2];
const pageId = process.argv[3];
const server = http.createServer((req, res) => {
  const expectedPath = `/pages/${encodeURIComponent(pageId)}/close`;
  if (req.method === "POST" && req.url === expectedPath) {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ schema: "elastos.browser.close-result/v1", page_id: pageId, closed: true }));
    return;
  }
  res.writeHead(404, { "content-type": "application/json" });
  res.end(JSON.stringify({ error: "not found" }));
});
server.listen(socketPath);
NODE
server_pid="$!"
for _ in {1..100}; do
  [[ -S "$socket_path" ]] && break
  sleep 0.02
done

node "$repo_root/scripts/browser-selkies-close-page.mjs" \
  --control-socket "$socket_path" \
  --page-id "$page_id" \
  --confirm-close \
  >"$tmp_dir/closed.json"

node -e '
  const fs = require("node:fs");
  const closed = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
  function fail(message) {
    console.error(message);
    process.exit(1);
  }
  if (closed.schema !== "elastos.browser.selkies-close-page/v1" || closed.ok !== true || closed.dry_run !== false) {
    fail("confirmed close must emit a close-page receipt");
  }
  if (closed.response?.schema !== "elastos.browser.close-result/v1" || closed.response?.closed !== true) {
    fail("confirmed close must preserve the Selkies close response");
  }
' "$tmp_dir/closed.json"

printf '{"schema":"elastos.browser.selkies-close-page-smoke/v1","ok":true,"dry_run_safe":true,"confirm_required":true,"confirmed_close":true}\n'
