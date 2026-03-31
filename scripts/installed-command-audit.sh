#!/usr/bin/env bash
set -euo pipefail

ELASTOS_BIN="${1:-${ELASTOS_AUDIT_BIN:-}}"
if [[ -z "${ELASTOS_BIN}" ]]; then
  ELASTOS_BIN="$(command -v elastos || true)"
fi

if [[ -z "${ELASTOS_BIN}" ]]; then
  echo "[installed-command-audit] no installed elastos binary found" >&2
  exit 2
fi

TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "$TMP_ROOT"' EXIT
export HOME="$TMP_ROOT/home"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME"
printf '# installed-command-audit\n' > "$TMP_ROOT/share.txt"

FAILURES=0

run_case() {
  local kind="$1"
  local name="$2"
  local pattern="${3:-}"
  shift 3

  local out="$TMP_ROOT/out.txt"
  local err="$TMP_ROOT/err.txt"
  local rc=0

  echo "[installed-command-audit] $name"
  set +e
  "$@" >"$out" 2>"$err"
  rc=$?
  set -e

  case "$kind" in
    ok)
      if [[ $rc -ne 0 ]]; then
        echo "[installed-command-audit] FAIL ($name): command exited $rc" >&2
        cat "$out" >&2 || true
        cat "$err" >&2 || true
        FAILURES=$((FAILURES + 1))
        return
      fi
      ;;
    fail)
      if [[ $rc -eq 0 ]]; then
        echo "[installed-command-audit] FAIL ($name): expected failure, command succeeded" >&2
        cat "$out" >&2 || true
        cat "$err" >&2 || true
        FAILURES=$((FAILURES + 1))
        return
      fi
      ;;
    fail-fast)
      if [[ $rc -eq 0 ]]; then
        echo "[installed-command-audit] FAIL ($name): expected failure, command succeeded" >&2
        cat "$out" >&2 || true
        cat "$err" >&2 || true
        FAILURES=$((FAILURES + 1))
        return
      fi
      if [[ $rc -eq 124 ]]; then
        echo "[installed-command-audit] FAIL ($name): command hung" >&2
        cat "$out" >&2 || true
        cat "$err" >&2 || true
        FAILURES=$((FAILURES + 1))
        return
      fi
      ;;
    *)
      echo "[installed-command-audit] internal error: unknown kind '$kind'" >&2
      exit 99
      ;;
  esac

  if [[ -n "$pattern" ]] && ! grep -Eq "$pattern" "$out" "$err"; then
    echo "[installed-command-audit] FAIL ($name): expected pattern '$pattern' not found" >&2
    cat "$out" >&2 || true
    cat "$err" >&2 || true
    FAILURES=$((FAILURES + 1))
  fi
}

run_case ok "version" "" "$ELASTOS_BIN" --version
run_case ok "root help exposes pc2" "pc2" "$ELASTOS_BIN" --help
run_case ok "root help exposes webspace" "webspace" "$ELASTOS_BIN" --help
run_case ok "root help exposes identity" "identity" "$ELASTOS_BIN" --help
run_case ok "pc2 help" "" "$ELASTOS_BIN" pc2 --help
run_case ok "identity help" "" "$ELASTOS_BIN" identity --help
run_case ok "identity nickname help" "" "$ELASTOS_BIN" identity nickname --help
run_case ok "webspace help" "" "$ELASTOS_BIN" webspace --help
run_case ok "site help" "" "$ELASTOS_BIN" site --help
run_case ok "site publish help" "" "$ELASTOS_BIN" site publish --help
run_case ok "site activate help" "" "$ELASTOS_BIN" site activate --help
run_case ok "site channels help" "" "$ELASTOS_BIN" site channels --help
run_case ok "config show on empty home is explicit" "No config file found|# \\(empty config file\\)" "$ELASTOS_BIN" config show
run_case ok "site path prints rooted path" "localhost://MyWebSite" "$ELASTOS_BIN" site path
run_case ok "shares list on empty home is explicit" "No shares yet" "$ELASTOS_BIN" shares list
run_case fail-fast \
  "share without extras fails clearly" \
  "ipfs-provider not found|elastos setup --with kubo --with ipfs-provider" \
  "$ELASTOS_BIN" share "$TMP_ROOT/share.txt"
run_case fail-fast \
  "open without extras fails clearly" \
  "ipfs-provider not found|elastos setup --with kubo --with ipfs-provider" \
  timeout 15s "$ELASTOS_BIN" open elastos://QmU8x9HMWetGzfnXLe4CriiocGuzvSLr9NJ1RwDp6MaWX6

if [[ $FAILURES -ne 0 ]]; then
  echo "[installed-command-audit] FAIL ($FAILURES cases)" >&2
  exit 1
fi

echo "[installed-command-audit] OK"
