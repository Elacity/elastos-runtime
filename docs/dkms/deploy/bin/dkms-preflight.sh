#!/usr/bin/env bash
#
# dKMS deployment preflight checker — grounded entirely in the real capsule surfaces:
#   * node identity     : `dkms-authority provision` (OFFLINE) -> seal_verifying_key_b64 / seal_recipient_pub_b64
#                         (capsules/dkms-authority/src/main.rs::run_provision, deterministic from the master store)
#   * node env          : DKMS_AUTHORITY_{LISTEN,KEY_STORE,ALLOWED_CALLERS,OPERATOR_VK}
#   * descriptor schema : elastos.dkms.authority/v2 (validated via dkms-validate-descriptor.py)
#   * reachability      : a raw TCP connect to each published tcp:HOST:PORT endpoint
#
# It performs NO secret operations and prints NO secrets. Run the relevant subcommand on the
# relevant host.
#
# Subcommands:
#   identity   NODE_BIN STORE_PATH
#       Provision/read this node's STABLE public identity. Run on each NODE host. Prints
#       {verifying_key_b64, recipient_pub_b64} you paste into the descriptor. Idempotent: the
#       same store always yields the same identity (it creates the master on first run).
#
#   node       ENV_FILE NODE_BIN [--expect-caller VK_B64] [--expect-operator VK_B64]
#       Validate a node host's env-file + binary before `systemctl start`. Run on each NODE host.
#
#   runtime    DESCRIPTOR_JSON INIT_CONFIG_JSON [--require-tcp]
#       Validate the runtime's descriptor + key-provider init config, and probe every node
#       endpoint for reachability. Run on the RUNTIME host.
#
# Exit: 0 = all checks passed, 1 = a check failed, 2 = usage error.

set -uo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
VALIDATOR="${HERE}/dkms-validate-descriptor.py"

green() { printf "  \033[32mPASS\033[0m  %s\n" "$1"; }
red()   { printf "  \033[31mFAIL\033[0m  %s\n" "$1"; FAILED=1; }
info()  { printf "  ----  %s\n" "$1"; }
FAILED=0

have() { command -v "$1" >/dev/null 2>&1; }

# Connect to tcp:HOST:PORT (best-effort, no payload). Uses bash /dev/tcp if available, else nc.
tcp_probe() {
  local hostport="${1#tcp:}"
  local host="${hostport%:*}" port="${hostport##*:}"
  if (exec 3<>"/dev/tcp/${host}/${port}") 2>/dev/null; then exec 3>&- 3<&- ; return 0; fi
  if have nc; then nc -z -w 3 "$host" "$port" >/dev/null 2>&1; return $?; fi
  return 1
}

# Read a node's published identity via the OFFLINE `provision` subcommand (DKMS-7): identity is
# created/loaded from the operator-owned state root, never over the wire.
read_identity() {
  local bin="$1" store="$2"
  DKMS_AUTHORITY_KEY_STORE="$store" "$bin" provision 2>/dev/null | tail -n 1
}

cmd_identity() {
  local bin="${1:-}" store="${2:-}"
  [[ -n "$bin" && -n "$store" ]] || { echo "usage: dkms-preflight.sh identity NODE_BIN STORE_PATH" >&2; exit 2; }
  echo "== node identity (offline provision; operator state root) =="
  [[ -x "$bin" ]] || { red "node binary $bin is not executable"; return 1; }
  green "node binary present: $("$bin" --version 2>/dev/null || basename "$bin")"
  local out; out="$(read_identity "$bin" "$store")"
  local vk rp
  vk="$(echo "$out" | jq -r '.seal_verifying_key_b64 // empty' 2>/dev/null)"
  rp="$(echo "$out" | jq -r '.seal_recipient_pub_b64 // empty' 2>/dev/null)"
  [[ -n "$vk" && "$vk" != null ]] || { red "no seal_verifying_key_b64 in provision response: $(echo "$out" | head -c 200)"; return 1; }
  [[ -n "$rp" && "$rp" != null ]] || { red "no seal_recipient_pub_b64 in provision response"; return 1; }
  green "store readable, identity is stable: $store"
  echo
  echo "Paste this node block into the descriptor (set authority_endpoint to this node's tcp: address):"
  jq -n --arg vk "$vk" --arg rp "$rp" \
    '{verifying_key_b64:$vk, recipient_pub_b64:$rp, authority_endpoint:"tcp:REPLACE_WITH_THIS_NODE_IP:REPLACE_PORT"}'
}

cmd_node() {
  local envf="${1:-}" bin="${2:-}"; shift 2 2>/dev/null || true
  local expect_caller="" expect_operator=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --expect-caller) expect_caller="${2:-}"; shift 2 ;;
      --expect-operator) expect_operator="${2:-}"; shift 2 ;;
      *) echo "unknown flag: $1" >&2; exit 2 ;;
    esac
  done
  [[ -n "$envf" && -n "$bin" ]] || { echo "usage: dkms-preflight.sh node ENV_FILE NODE_BIN [--expect-caller VK] [--expect-operator VK]" >&2; exit 2; }
  echo "== node host preflight =="
  [[ -f "$envf" ]] || { red "env file $envf not found"; return 1; }
  green "env file present: $envf"
  # shellcheck disable=SC1090
  set -a; source "$envf"; set +a

  [[ -x "$bin" ]] && green "node binary present: $bin" || red "node binary $bin is not executable"

  local listen="${DKMS_AUTHORITY_LISTEN:-}"
  if [[ -z "$listen" ]]; then red "DKMS_AUTHORITY_LISTEN unset (the daemon would fall back to stdio, NOT a listener)"
  elif [[ "$listen" == tcp:* ]]; then green "DKMS_AUTHORITY_LISTEN is a TCP endpoint: $listen"
  else info "DKMS_AUTHORITY_LISTEN is a unix path: $listen (use tcp: for a remote node)"; fi

  local store="${DKMS_AUTHORITY_KEY_STORE:-}"
  [[ -n "$store" ]] && green "DKMS_AUTHORITY_KEY_STORE set: $store" || red "DKMS_AUTHORITY_KEY_STORE unset (startup fails closed: no master store)"
  if [[ -n "$store" && -e "$store" ]]; then
    local mode; mode="$(stat -f '%Lp' "$store" 2>/dev/null || stat -c '%a' "$store" 2>/dev/null)"
    [[ "$mode" == "600" ]] && green "master store mode is 0600" || red "master store mode is $mode (MUST be 0600 — it holds the node's master seed)"
  fi

  # DKMS-8: caller policy is fail-closed and EXPLICIT. Detect the distinct modes the daemon itself
  # now distinguishes at startup: an allow-list, explicit anonymous, or a misconfiguration that
  # ABORTS startup (unset-without-opt-in, empty, or both set at once). This preflight only surfaces
  # the intent; the daemon is the authority and refuses to bind on any misconfiguration.
  local allowed="${DKMS_AUTHORITY_ALLOWED_CALLERS:-}"
  local allow_anon="${DKMS_AUTHORITY_ALLOW_ANONYMOUS:-}"
  local trimmed; trimmed="$(tr -d '[:space:],' <<<"$allowed")"
  if [[ -n "$allowed" && "$allow_anon" == "1" ]]; then
    red "DKMS_AUTHORITY_ALLOWED_CALLERS and DKMS_AUTHORITY_ALLOW_ANONYMOUS=1 are BOTH set (contradictory — the node fails closed on startup; choose one)"
  elif [[ -n "$allowed" ]]; then
    if [[ -z "$trimmed" ]]; then
      red "DKMS_AUTHORITY_ALLOWED_CALLERS is set but EMPTY (the node fails closed on startup — a malformed allow-list never degrades to anonymous)"
    else
      green "DKMS_AUTHORITY_ALLOWED_CALLERS set ($(awk -F, '{c=0; for(i=1;i<=NF;i++) if($i ~ /[^[:space:]]/) c++; print c}' <<<"$allowed") caller(s))"
      if [[ -n "$expect_caller" ]]; then
        [[ ",$allowed," == *",$expect_caller,"* ]] && green "expected runtime caller is allow-listed" || red "expected runtime caller VK is NOT in the allow-list"
      fi
    fi
  elif [[ "$allow_anon" == "1" ]]; then
    info "DKMS_AUTHORITY_ALLOW_ANONYMOUS=1 (EXPLICIT anonymous: node serves ANY well-formed caller — intended only for dev/open meshes, not an allow-listed production node)"
  else
    red "no caller policy configured (the node fails closed on startup — set DKMS_AUTHORITY_ALLOWED_CALLERS to the runtime VK, or DKMS_AUTHORITY_ALLOW_ANONYMOUS=1 to opt into anonymous)"
  fi

  local op="${DKMS_AUTHORITY_OPERATOR_VK:-}"
  if [[ -z "$op" ]]; then red "DKMS_AUTHORITY_OPERATOR_VK unset (lifecycle ops — rotate/revoke/reconfigure/DKG — would be DISABLED)"
  else
    green "DKMS_AUTHORITY_OPERATOR_VK pinned"
    [[ -n "$expect_operator" && "$op" != "$expect_operator" ]] && red "operator VK does not match the expected operator identity"
  fi

  if [[ -n "$store" && -x "$bin" ]]; then
    local out vk; out="$(read_identity "$bin" "$store")"
    vk="$(echo "$out" | jq -r '.data.seal_verifying_key_b64 // .seal_verifying_key_b64 // empty' 2>/dev/null)"
    [[ -n "$vk" ]] && green "node identity reads cleanly from the store" || red "could not read node identity from the store"
  fi
}

cmd_runtime() {
  local desc="${1:-}" initcfg="${2:-}"; shift 2 2>/dev/null || true
  local require_tcp=""
  [[ "${1:-}" == "--require-tcp" ]] && require_tcp="--require-tcp"
  [[ -n "$desc" && -n "$initcfg" ]] || { echo "usage: dkms-preflight.sh runtime DESCRIPTOR_JSON INIT_CONFIG_JSON [--require-tcp]" >&2; exit 2; }
  echo "== runtime host preflight =="

  echo "-- descriptor schema --"
  if python3 "$VALIDATOR" "$desc" $require_tcp; then green "descriptor validates against elastos.dkms.authority/v2"; else red "descriptor FAILED validation"; fi

  echo "-- key-provider init config --"
  if [[ -f "$initcfg" ]]; then
    local backend seed dpath
    backend="$(jq -r '.config.backend // .backend // empty' "$initcfg" 2>/dev/null)"
    seed="$(jq -r '.config.dkms_caller_seed_b64 // .dkms_caller_seed_b64 // empty' "$initcfg" 2>/dev/null)"
    dpath="$(jq -r '.config.dkms_authority_descriptor // .dkms_authority_descriptor // empty' "$initcfg" 2>/dev/null)"
    [[ "$backend" == "dkms" ]] && green "backend is dkms" || red "backend is '$backend' (expected dkms)"
    [[ -n "$seed" ]] && green "dkms_caller_seed_b64 present (the runtime's allow-listed caller identity)" || red "dkms_caller_seed_b64 missing (the node would treat the runtime as anonymous)"
    [[ -n "$dpath" ]] && green "dkms_authority_descriptor path set: $dpath" || red "dkms_authority_descriptor path missing"
  else
    red "init config $initcfg not found"
  fi

  echo "-- node reachability --"
  local eps; eps="$(jq -r '(.threshold.nodes[]?.authority_endpoint) // .authority_endpoint' "$desc" 2>/dev/null | sort -u)"
  if [[ -z "$eps" ]]; then red "no endpoints found in descriptor"; else
    while IFS= read -r ep; do
      [[ -z "$ep" || "$ep" == null ]] && continue
      if [[ "$ep" == tcp:* ]]; then
        if tcp_probe "$ep"; then green "reachable: $ep"; else red "NOT reachable: $ep (node down, firewall, or WireGuard not up)"; fi
      else
        [[ -S "$ep" ]] && green "unix socket present: $ep" || info "unix endpoint (local only): $ep"
      fi
    done <<< "$eps"
  fi
}

main() {
  local sub="${1:-}"; shift || true
  case "$sub" in
    identity) cmd_identity "$@" ;;
    node)     cmd_node "$@" ;;
    runtime)  cmd_runtime "$@" ;;
    *) echo "usage: dkms-preflight.sh {identity|node|runtime} ..." >&2; exit 2 ;;
  esac
  echo
  if [[ $FAILED -eq 0 ]]; then echo "PREFLIGHT: PASS"; exit 0; else echo "PREFLIGHT: FAIL" >&2; exit 1; fi
}

main "$@"
