#!/usr/bin/env bash
# Stop the runtime this run started and remove its data home. Evidence stays.
set -euo pipefail

ENV_FILE="${1:-}"
if [[ -z "$ENV_FILE" || ! -f "$ENV_FILE" ]]; then
  echo "usage: cleanup.sh /path/to/run.env" >&2
  exit 2
fi
# shellcheck disable=SC1090
source "$ENV_FILE"

if [[ -d "/proc/$PID" ]]; then
  cmdline="$(tr '\0' ' ' <"/proc/$PID/cmdline")"
  case "$cmdline" in
    "$BIN serve --addr 127.0.0.1:$PORT"*)
      children="$(ps -o pid= --ppid "$PID" | awk '{print $1}')"
      if [[ -n "$children" ]]; then
        kill $children 2>/dev/null || true
      fi
      kill "$PID" 2>/dev/null || true
      for _ in 1 2 3 4 5 6 7 8 9 10; do
        if [[ ! -d "/proc/$PID" ]]; then
          break
        fi
        sleep 0.2
      done
      if [[ -d "/proc/$PID" ]]; then
        kill -9 "$PID" 2>/dev/null || true
      fi
      ;;
    *)
      echo "cleanup: refused to kill pid $PID because it is not this run's runtime" >&2
      exit 1
      ;;
  esac
fi

rm -rf "$STATE"
if [[ -d "$EVIDENCE" ]]; then
  echo "evidence kept at $EVIDENCE"
fi
