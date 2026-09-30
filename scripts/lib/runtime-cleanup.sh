#!/usr/bin/env bash

_elastos_installer_helper() (
    # Isolate the installer's shell options and bootstrap variables.
    source "$(dirname "${BASH_SOURCE[0]}")/../install.sh"
    "$@"
)

elastos_runtime_data_dir() {
    _elastos_installer_helper installer_data_dir "$@"
}

cleanup_elastos_runtime_home() {
    local home_dir="$1"
    local data_dir="${2:-}"
    local binary="${3:-${home_dir}/.local/bin/elastos}"
    local scan_binary=false
    if [[ -z "$data_dir" ]]; then
        data_dir="$(elastos_runtime_data_dir "$home_dir" "${home_dir}/xdg-data")" || return 1
    fi
    # Detect unrecorded users of the private installed binary before cleanup.
    # A branch override can be shared, so cleanup uses only this home's coords.
    if [[ "$binary" == "${home_dir}/.local/bin/elastos" ]]; then
        scan_binary=true
    fi
    _elastos_installer_helper installer_runtime_control "$data_dir" "$binary" "$scan_binary"
}
