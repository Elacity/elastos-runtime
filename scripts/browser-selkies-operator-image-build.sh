#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image_tag="${BROWSER_SELKIES_OPERATOR_IMAGE:-elastos/browser-selkies-runtime-target:dev}"

cd "$repo_root"

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to build the Browser Selkies operator image" >&2
  exit 1
fi

docker build \
  -f deploy/browser-selkies-runtime-target/Dockerfile \
  -t "$image_tag" \
  "$repo_root"

node - <<NODE
console.log(JSON.stringify({
  ok: true,
  schema: "elastos.browser.selkies-operator-image/v1",
  image: "$image_tag",
  dockerfile: "deploy/browser-selkies-runtime-target/Dockerfile"
}));
NODE
