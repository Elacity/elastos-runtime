#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
original_home="$HOME"
export CARGO_HOME="${CARGO_HOME:-${original_home}/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-${original_home}/.rustup}"
export ELASTOS_CARGO_BIN="$(command -v cargo)"
export ELASTOS_NODE_BIN="$(command -v node)"

functions_file="$tmp_dir/setup-source-home-functions.sh"
awk '
  /^ROOT=/ {
    print "ROOT=\"$repo_root\""
    next
  }
  /^echo "\[setup-source-home\] repo:/ { exit }
  { print }
' "$repo_root/scripts/setup-source-home.sh" >"$functions_file"

fake_verifier="$tmp_dir/fake-elastos"
cat >"$fake_verifier" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${FAKE_VERIFY_LOG:?}"
if [[ "$#" -ne 4 || "$1" != "collaboration-config" || "$2" != "verify" || "$3" != "--input" ]]; then
  echo "unexpected collaboration-config verifier invocation" >&2
  exit 70
fi
python3 - "$4" <<'PY'
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
mode = path.stat().st_mode & 0o777
if mode != 0o600:
    raise SystemExit(71)
if path.read_bytes() not in (b"valid-config-a", b"valid-config-b"):
    raise SystemExit(72)
PY
EOF
chmod 755 "$fake_verifier"

runner="$tmp_dir/run-install.sh"
cat >"$runner" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
functions_file="$1"
repo_root="$2"
data_dir="$3"
fake_verifier="$4"
set --
source "$functions_file"
ROOT="$repo_root"
DATA_DIR="$data_dir"
cargo_built_binary_path() {
  printf '%s\n' "$fake_verifier"
}
ensure_owner_only_data_dir
validate_collaboration_startup_mode
verify_collaboration_startup_config_input
install_collaboration_startup_config
EOF
chmod 755 "$runner"

verify_log="$tmp_dir/verify.log"
: >"$verify_log"

case "$(uname -s)" in
  Darwin) data_dir="$tmp_dir/home/Library/Application Support/elastos" ;;
  Linux) data_dir="$tmp_dir/xdg/elastos" ;;
  *) echo "unsupported setup-source-home smoke host: $(uname -s)" >&2; exit 2 ;;
esac

run_install() {
  local mode="$1"
  local input_path="${2:-}"
  if [[ -n "$input_path" ]]; then
    env \
      HOME="$tmp_dir/home" \
      XDG_DATA_HOME="$tmp_dir/xdg" \
      ELASTOS_COLLABORATION_STARTUP_MODE="$mode" \
      ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT="$input_path" \
      FAKE_VERIFY_LOG="$verify_log" \
      "$runner" "$functions_file" "$repo_root" "$data_dir" "$fake_verifier"
  else
    env \
      HOME="$tmp_dir/home" \
      XDG_DATA_HOME="$tmp_dir/xdg" \
      ELASTOS_COLLABORATION_STARTUP_MODE="$mode" \
      FAKE_VERIFY_LOG="$verify_log" \
      "$runner" "$functions_file" "$repo_root" "$data_dir" "$fake_verifier"
  fi
}

dest="$data_dir/collaboration-network-v1.json"
valid_a="$tmp_dir/valid-a.json"
valid_b="$tmp_dir/valid-b.json"
invalid="$tmp_dir/invalid.json"
wrong_mode="$tmp_dir/wrong-mode.json"
printf 'valid-config-a' >"$valid_a"
printf 'valid-config-b' >"$valid_b"
printf 'invalid-config' >"$invalid"
printf 'valid-config-a' >"$wrong_mode"
chmod 600 "$valid_a" "$valid_b" "$invalid"
chmod 644 "$wrong_mode"

if run_install configured; then
  echo "setup-source-home accepted configured mode without a signed startup config input" >&2
  exit 1
fi
if [[ -e "$dest" ]]; then
  echo "missing configured startup config input mutated the destination" >&2
  exit 1
fi

run_install isolated
if [[ -e "$dest" ]]; then
  echo "setup-source-home mutated the destination in explicit isolated mode" >&2
  exit 1
fi

if run_install configured "$tmp_dir/missing.json"; then
  echo "setup-source-home accepted a missing collaboration config input" >&2
  exit 1
fi
if [[ -e "$dest" ]]; then
  echo "missing collaboration config input mutated the destination" >&2
  exit 1
fi

if run_install configured "$invalid"; then
  echo "setup-source-home accepted an invalid collaboration config input" >&2
  exit 1
fi
if [[ -e "$dest" ]]; then
  echo "invalid collaboration config input mutated the destination" >&2
  exit 1
fi

if run_install configured "$wrong_mode"; then
  echo "setup-source-home accepted a non-owner-only collaboration config input" >&2
  exit 1
fi
if [[ -e "$dest" ]]; then
  echo "non-owner-only collaboration config input mutated the destination" >&2
  exit 1
fi

run_install configured "$valid_a"
if [[ ! -f "$dest" ]]; then
  echo "setup-source-home did not install the collaboration config at the exact destination" >&2
  exit 1
fi
cmp "$valid_a" "$dest"

python3 - "$data_dir" "$dest" <<'PY'
import pathlib
import sys

data_dir = pathlib.Path(sys.argv[1])
path = pathlib.Path(sys.argv[2])
if (data_dir.stat().st_mode & 0o777) != 0o700:
    raise SystemExit("installed collaboration data root is not owner-only")
if (path.stat().st_mode & 0o777) != 0o600:
    raise SystemExit("installed collaboration config is not owner-only")
PY

installed_paths="$(find "$tmp_dir" -name 'collaboration-network-v1.json' -print)"
if [[ "$installed_paths" != "$dest" ]]; then
  echo "setup-source-home installed the collaboration config outside the explicit data root" >&2
  exit 1
fi

run_install configured "$valid_a"
cmp "$valid_a" "$dest"

if run_install configured "$valid_b"; then
  echo "setup-source-home replaced an existing collaboration config with different bytes" >&2
  exit 1
fi
cmp "$valid_a" "$dest"

if run_install isolated; then
  echo "setup-source-home accepted isolated mode on a data root that already has a collaboration config" >&2
  exit 1
fi

if env \
  HOME="$tmp_dir/home" \
  XDG_DATA_HOME="$tmp_dir/xdg" \
  ELASTOS_COLLABORATION_STARTUP_MODE=configured \
  ELASTOS_COLLABORATION_STARTUP_CONFIG_INPUT="$valid_a" \
  SETUP_SOURCE_HOME_CONFIG_ONLY=1 \
  "$repo_root/scripts/setup-source-home.sh" >/dev/null 2>&1; then
  echo "setup-source-home accepted a collaboration config input in config-only mode" >&2
  exit 1
fi

python3 - "$verify_log" "$dest" <<'PY'
import pathlib
import sys

log_path = pathlib.Path(sys.argv[1])
dest = pathlib.Path(sys.argv[2])
lines = [line.strip() for line in log_path.read_text().splitlines() if line.strip()]
expected = [
    "collaboration-config verify --input",
    "collaboration-config verify --input",
    "collaboration-config verify --input",
    "collaboration-config verify --input",
    "collaboration-config verify --input",
]
if len(lines) != len(expected):
    raise SystemExit(f"unexpected verifier invocation count: {len(lines)}")
for line in lines:
    if not line.startswith("collaboration-config verify --input "):
        raise SystemExit(f"unexpected verifier invocation: {line}")
if not dest.exists():
    raise SystemExit("installed collaboration config is missing")
PY

printf '%s\n' '{"schema":"elastos.setup-source-home.collaboration-config-smoke/v1","ok":true}'
