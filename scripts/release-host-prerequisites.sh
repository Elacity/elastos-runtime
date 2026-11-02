#!/usr/bin/env bash
# Host packages for native release preparation, including the Browser helper
# source builds in scripts/release-upstream-assets.py. release-package.yml and
# CI's release-upstream-assets job both install exactly this list.
# Usage: release-host-prerequisites.sh linux|darwin
set -euo pipefail

case "${1:-}" in
    linux)
        apt=(sudo apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=30)
        "${apt[@]}" update
        "${apt[@]}" install -y build-essential cmake coturn e2fsprogs ffmpeg musl-tools nasm pkg-config autoconf automake libtool clang libclang-dev linux-libc-dev zstd
        ;;
    darwin)
        brew install e2fsprogs coturn pkg-config openssl@3 autoconf automake libtool zstd
        ;;
    *)
        echo "usage: $0 linux|darwin" >&2
        exit 2
        ;;
esac
