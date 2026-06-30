#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat >&2 <<'USAGE'
Usage:
  scripts/setup-source-home-browser-artifacts.sh --data-dir PATH --platform PLATFORM

Options:
  --artifact-data-dir PATH  Source ElastOS data dir containing Browser VM artifacts.
                            Defaults to ELASTOS_BROWSER_VM_ARTIFACT_DATA_DIR
                            and, for managed runtimes, the parent data dir.

Links missing Browser VM substrate artifacts into a source-home data dir without
copying large VM images or replacing real files. This keeps managed runtimes
renewable while preserving the operator-owned top-level artifact store.
USAGE
}

data_dir=""
platform=""
artifact_data_dir="${ELASTOS_BROWSER_VM_ARTIFACT_DATA_DIR:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --data-dir)
      data_dir="${2:-}"
      shift 2
      ;;
    --platform)
      platform="${2:-}"
      shift 2
      ;;
    --artifact-data-dir)
      artifact_data_dir="${2:-}"
      shift 2
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage
      exit 2
      ;;
  esac
done

if [[ -z "$data_dir" || "$data_dir" != /* ]]; then
  echo "--data-dir must be an absolute path" >&2
  exit 2
fi
if [[ -z "$platform" ]]; then
  echo "--platform is required" >&2
  exit 2
fi
if [[ -n "$artifact_data_dir" && "$artifact_data_dir" != /* ]]; then
  echo "--artifact-data-dir must be an absolute path" >&2
  exit 2
fi

candidate_dirs=()
candidate_dir_count=0
add_candidate_dir() {
  local candidate="$1"
  if [[ -z "$candidate" || "$candidate" == "$data_dir" || ! -d "$candidate" ]]; then
    return
  fi
  if (( candidate_dir_count > 0 )); then
    local existing
    for existing in "${candidate_dirs[@]}"; do
      if [[ "$existing" == "$candidate" ]]; then
        return
      fi
    done
  fi
  candidate_dirs+=("$candidate")
  candidate_dir_count=$((candidate_dir_count + 1))
}

add_candidate_dir "$artifact_data_dir"
case "$data_dir" in
  */managed-runtimes/*)
    add_candidate_dir "${data_dir%%/managed-runtimes/*}"
    ;;
esac

linked=0
skipped_existing=0
missing=0

link_artifact() {
  local dest_rel="$1"
  shift
  local dest="${data_dir}/${dest_rel}"
  local source_dir source_rel source=""

  if [[ -e "$dest" && ! -L "$dest" ]]; then
    skipped_existing=$((skipped_existing + 1))
    return
  fi
  if [[ -L "$dest" && -e "$dest" ]]; then
    skipped_existing=$((skipped_existing + 1))
    return
  fi

  if (( candidate_dir_count > 0 )); then
    for source_dir in "${candidate_dirs[@]}"; do
      for source_rel in "$@"; do
        if [[ -e "${source_dir}/${source_rel}" ]]; then
          source="${source_dir}/${source_rel}"
          break 2
        fi
      done
    done
  fi

  if [[ -z "$source" ]]; then
    missing=$((missing + 1))
    return
  fi

  mkdir -p "$(dirname "$dest")"
  ln -sfn "$source" "$dest"
  linked=$((linked + 1))
  echo "[setup-source-home] linked Browser VM artifact: ${dest} -> ${source}"
}

link_artifact "bin/vmlinux" "bin/vmlinux"
link_artifact "browser-vm/rootfs.ext4" "browser-vm/rootfs.ext4"

case "$platform" in
  linux-*)
    link_artifact "bin/crosvm" "bin/crosvm"
    link_artifact "browser-vm/initrd" "browser-vm/initrd"
    ;;
  darwin-arm64)
    link_artifact "bin/initrd" "bin/initrd"
    ;;
esac

candidate_dirs_text=""
if (( candidate_dir_count > 0 )); then
  printf -v candidate_dirs_text '%s\n' "${candidate_dirs[@]}"
fi

CANDIDATE_DIRS="${candidate_dirs_text}" python3 - "$platform" "$data_dir" "$linked" "$skipped_existing" "$missing" <<'PY'
import json
import os
import sys

platform, data_dir, linked, skipped_existing, missing = sys.argv[1:]
candidate_dirs = [
    entry for entry in os.environ.get("CANDIDATE_DIRS", "").splitlines() if entry
]
print(json.dumps({
    "schema": "elastos.setup-source-home.browser-artifacts/v1",
    "ok": True,
    "platform": platform,
    "data_dir": data_dir,
    "candidate_dirs": candidate_dirs,
    "linked": int(linked),
    "skipped_existing": int(skipped_existing),
    "missing": int(missing),
}, separators=(",", ":")))
PY
