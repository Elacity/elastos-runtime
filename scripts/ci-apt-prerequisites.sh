#!/usr/bin/env bash
# Ubuntu's signed indexes and package hashes own authentication, including cache hits.
set -euo pipefail

# A mirror hiccup should not fail a long job: three bounded attempts with
# growing pauses (15s, then 30s) give a flaky mirror time to recover.
retry() {
    local attempt
    for attempt in 1 2 3; do
        if sudo timeout --kill-after=5s 90s "$@"; then
            return 0
        fi
        echo "apt attempt ${attempt}/3 failed: $*" >&2
        if [[ "$attempt" != 3 ]]; then sleep $((attempt * 15)); fi
    done
    return 1
}

read -r -a packages <<< "$CI_APT_PACKAGES"
# Read only Ubuntu sources. Runner-added repositories are outside this step.
sources="$(mktemp -d)"
trap 'rm -rf "$sources"' EXIT
for file in /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources; do
    if [[ -f "$file" ]]; then cp "$file" "$sources/$(basename "$file")"; fi
done
[[ -n "$(ls -A "$sources")" ]]
# The runner image's Ubuntu signing key remains the trust root. A cache can
# supply bytes, but apt verifies them against fresh authenticated indexes.
options=(-o "Dir::Etc::sourcelist=/dev/null" -o "Dir::Etc::sourceparts=$sources"
         -o Acquire::Retries=0 -o Acquire::http::Timeout=20 -o Acquire::https::Timeout=20
         -o Acquire::AllowInsecureRepositories=false -o APT::Get::AllowUnauthenticated=false
         -o APT::Keep-Downloaded-Packages=true -o Binary::apt::APT::Keep-Downloaded-Packages=true
         -o APT::Update::Error-Mode=any)
sudo install -d -o _apt -g root -m 700 /var/cache/apt/archives/partial
retry apt-get "${options[@]}" update
verify_archives() {
    # apt can reuse an existing .deb by size. Check the full SHA-256 against
    # the authenticated package index before apt sees restored archive bytes.
    sudo python3 - "$@" "${options[@]}" <<'PY'
import hashlib
from pathlib import Path
import subprocess
import sys

strict, options = sys.argv[1] == 'strict', sys.argv[2:]
for archive in Path('/var/cache/apt/archives').glob('*.deb'):
    try:
        if archive.is_symlink() or not archive.is_file():
            raise ValueError('apt archive requires a regular file')
        package, version, arch = subprocess.check_output([
            'dpkg-deb', '--show', '--showformat=${Package}\n${Version}\n${Architecture}\n',
            str(archive)], text=True).splitlines()
        index = subprocess.check_output(['apt-cache', *options, 'show', f'{package}:{arch}={version}'], text=True)
        hashes = [line.removeprefix('SHA256: ') for line in index.splitlines() if line.startswith('SHA256: ')]
        value = hashlib.sha256()
        with archive.open('rb') as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                value.update(chunk)
        if value.hexdigest() in hashes:
            continue
    except (subprocess.CalledProcessError, ValueError):
        pass
    archive.unlink()
    print(f'Removed unverified apt archive: {archive.name}', file=sys.stderr)
    if strict:
        raise SystemExit('Downloaded apt archive failed its signed-index SHA-256 check')
PY
}
verify_archives restore
archives_before="$(find /var/cache/apt/archives -maxdepth 1 -type f -name '*.deb' | sort)"
retry apt-get "${options[@]}" install --download-only -y --no-install-recommends "${packages[@]}"
verify_archives strict
archives_after="$(find /var/cache/apt/archives -maxdepth 1 -type f -name '*.deb' | sort)"
changed=false
if [[ "$archives_before" != "$archives_after" ]]; then changed=true; fi
printf 'changed=%s\n' "$changed" >> "$GITHUB_OUTPUT"
# Installation is local after the bounded download phase. It may need longer
# than a download attempt; --no-download keeps mirrors out of this phase.
sudo env DEBIAN_FRONTEND=noninteractive timeout --kill-after=5s 180s \
    apt-get "${options[@]}" install --no-download -y --no-install-recommends "${packages[@]}"
# actions/cache runs as the runner user; retain _apt's access to partial/.
sudo chown -R "$(id -u):$(id -g)" /var/cache/apt/archives
sudo install -d -o _apt -g root -m 700 /var/cache/apt/archives/partial
