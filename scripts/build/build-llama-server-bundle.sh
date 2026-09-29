#!/usr/bin/env bash
# Build and smoke-test the pinned CPU engine in Ubuntu 22.04 ARM64.
set -euo pipefail

if [[ ${1:-} == --help ]]; then
  echo "Usage: $0 NEW_OUTPUT_DIRECTORY"
  echo "Requires Ubuntu 22.04 ARM64, build-essential, cmake, git, curl, binutils, file and python3."
  exit 0
fi
[[ $# == 1 ]] || { echo "Usage: $0 NEW_OUTPUT_DIRECTORY" >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == aarch64 ]] || {
  echo "Run this build on Linux ARM64." >&2; exit 1;
}
[[ $(getconf GNU_LIBC_VERSION) == "glibc 2.35" ]] || {
  echo "The build requires the Ubuntu 22.04 glibc 2.35 baseline." >&2; exit 1;
}
[[ ! -e $1 ]] || { echo "Use a new output directory: $1" >&2; exit 1; }
for tool in cmake gcc g++ git curl readelf file python3 sha256sum dpkg-query tar gzip; do
  command -v "$tool" >/dev/null
done

tag=b10516
commit=b95502ba9aa0eb73a2f4fc8878d7fbe6a847a0b9
fixture_revision=9e6855bc4be717fca1ef21360a1db4b29d5c559a
fixture_sha=c4a3dd037301b6ecea31d6da37f5cd793ead920dd5ddfe6d589294628d6ce66a
mkdir -p "$1"
out=$(realpath "$1")
work=$(mktemp -d)
engine_pid=
cleanup() {
  if [[ -n $engine_pid ]]; then
    kill "$engine_pid" 2>/dev/null || true
    wait "$engine_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

src=$work/source
git init -q "$src"
git -C "$src" remote add origin https://github.com/ggml-org/llama.cpp.git
git -C "$src" fetch --quiet --depth=1 origin "$commit"
git -C "$src" checkout --quiet --detach FETCH_HEAD
[[ $(git -C "$src" rev-parse HEAD) == "$commit" ]]
export SOURCE_DATE_EPOCH
SOURCE_DATE_EPOCH=$(git -C "$src" show -s --format=%ct HEAD)

# Upstream explicitly supports these overrides in CMakeLists.txt and
# common/build-info.cpp.in. The release tag supplies the display number;
# the receipt records the full commit independently of shallow history.
cmake_args=(
  -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=ON
  -DCMAKE_C_COMPILER=gcc -DCMAKE_CXX_COMPILER=g++
  '-DCMAKE_INSTALL_RPATH=$ORIGIN' -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON
  -DLLAMA_BUILD_NUMBER=10516 "-DLLAMA_BUILD_COMMIT=$commit"
  -DGGML_NATIVE=OFF -DGGML_CPU_ARM_ARCH=armv8.2-a+dotprod+fp16
  -DGGML_BACKEND_DL=OFF -DGGML_OPENMP=OFF -DLLAMA_OPENSSL=OFF
  -DLLAMA_BUILD_UI=OFF -DLLAMA_USE_PREBUILT_UI=OFF
  -DLLAMA_BUILD_TESTS=OFF -DLLAMA_BUILD_EXAMPLES=OFF -DLLAMA_BUILD_APP=OFF
  "-DCMAKE_C_FLAGS=-ffile-prefix-map=$work=."
  "-DCMAKE_CXX_FLAGS=-ffile-prefix-map=$work=."
)
cmake -S "$src" -B "$work/build" "${cmake_args[@]}"
cmake --build "$work/build" --target llama-server --parallel 4

bundle=$work/stage/llama-$tag
mkdir -p "$bundle/licenses"
cp -a "$work/build/bin/llama-server" "$work/build/bin/"lib*.so* "$bundle/"
cp "$src/LICENSE" "$bundle/"
cp -a "$src/licenses/." "$bundle/licenses/"
# Keep standalone notices and full headers where upstream embeds the license.
while IFS= read -r path; do
  mkdir -p "$bundle/licenses/$(dirname "$path")"
  cp "$src/$path" "$bundle/licenses/$path"
done < <(git -C "$src" ls-files 'vendor/**/LICENSE*' \
  vendor/miniaudio/miniaudio.h vendor/sheredom/subprocess.h vendor/stb/stb_image.h)

{
  printf 'source_tag=%s\nsource_commit=%s\nsource_tree=%s\nsource_epoch=%s\n' \
    "$tag" "$commit" "$(git -C "$src" rev-parse 'HEAD^{tree}')" "$SOURCE_DATE_EPOCH"
  printf 'recipe_commit=%s\nrecipe_sha256=%s\ncontainer_image=%s\n' \
    "${RECIPE_COMMIT:-unrecorded}" "$(sha256sum "$0" | cut -d ' ' -f 1)" \
    "${BUILD_CONTAINER_IMAGE:-unrecorded}"
  printf 'cmake_flags='
  for flag in "${cmake_args[@]}"; do printf '%q ' "${flag//$work/\$WORK}"; done
  printf '\n'
  cat /etc/os-release
  uname -m
  getconf GNU_LIBC_VERSION
  gcc --version
  g++ --version
  cmake --version
  dpkg-query -W -f='${binary:Package}\t${Version}\n' | sort
  env -i PATH=/usr/bin:/bin "$bundle/llama-server" --version
} > "$bundle/BUILD-INFO.txt" 2>&1
cp "$bundle/BUILD-INFO.txt" "$out/build-info.txt"
find "$bundle" -type f -perm /111 -exec chmod 0755 {} +
find "$bundle" -type f ! -perm /111 -exec chmod 0644 {} +
find "$bundle" -type d -exec chmod 0755 {} +

archive=llama-$tag-bin-ubuntu22.04-arm64-cpu.tar.gz
tar -C "$work/stage" --sort=name --mtime="@$SOURCE_DATE_EPOCH" \
  --owner=0 --group=0 --numeric-owner --format=gnu -cf - "llama-$tag" \
  | gzip -9n > "$out/$archive"
(cd "$out" && sha256sum "$archive" > "$archive.sha256" && sha256sum -c "$archive.sha256")
mkdir "$work/unpacked"
tar -xzf "$out/$archive" -C "$work/unpacked"
bundle=$work/unpacked/llama-$tag

python3 - "$bundle" > "$out/elf-verification.txt" <<'PY'
import os, pathlib, re, stat, subprocess, sys
root = pathlib.Path(sys.argv[1])
limits = {"GLIBC": (2, 35), "GLIBCXX": (3, 4, 30), "CXXABI": (1, 3, 13)}
host_libraries = {"libc.so.6", "libm.so.6", "libstdc++.so.6", "libgcc_s.so.1", "ld-linux-aarch64.so.1"}
elf_count = 0
for path in sorted(root.rglob("*")):
    mode = path.lstat().st_mode
    if stat.S_ISLNK(mode):
        target = pathlib.PurePosixPath(os.readlink(path))
        assert not target.is_absolute() and ".." not in target.parts, f"unsafe symlink: {path}"
        assert path.exists() and path.resolve().is_relative_to(root), f"broken symlink: {path}"
        continue
    if stat.S_ISDIR(mode):
        continue
    assert stat.S_ISREG(mode) and path.stat().st_nlink == 1, f"unsupported file: {path}"
    with path.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            continue
    elf_count += 1
    header = subprocess.check_output(["readelf", "-h", str(path)], text=True)
    assert re.search(r"Class:\s+ELF64", header) and re.search(r"Machine:\s+AArch64", header), path
    dynamic = subprocess.check_output(["readelf", "-d", str(path)], text=True)
    assert re.search(r"\(RUNPATH\).*\[\$ORIGIN\]", dynamic), f"RUNPATH: {path}"
    for library in re.findall(r"\(NEEDED\).*\[(.+?)\]", dynamic):
        assert library in host_libraries or (root / library).is_file(), f"unbundled dependency: {library}"
    versions = subprocess.check_output(["readelf", "--version-info", str(path)], text=True)
    for family, number in re.findall(r"\b(GLIBCXX|GLIBC|CXXABI)_([0-9.]+)", versions):
        assert tuple(map(int, number.split("."))) <= limits[family], f"{path.name}: {family}_{number}"
    linked = subprocess.check_output(["ldd", str(path)], text=True, stderr=subprocess.STDOUT)
    assert "not found" not in linked, linked
    print(path.name, "\n" + dynamic + versions + linked)
assert elf_count > 1 and (root / "llama-server").is_file(), "engine and shared libraries required"
print("PASS: ARM64 ELF, relative links, $ORIGIN, dependency allowlist and symbol limits")
PY
env -i PATH=/usr/bin:/bin "$bundle/llama-server" --version > "$out/engine-version.txt" 2>&1
(cd "$bundle" && sha256sum llama-server) > "$out/engine.sha256"

# CI fetches one small fixture once; the archive contains only the engine.
curl --fail --location --proto '=https' --proto-redir '=https' \
  --retry 2 --connect-timeout 20 --max-time 300 \
  "https://huggingface.co/unsloth/SmolLM2-135M-Instruct-GGUF/resolve/$fixture_revision/SmolLM2-135M-Instruct-Q8_0.gguf" \
  -o "$work/smollm2.gguf"
printf '%s  %s\n' "$fixture_sha" "$work/smollm2.gguf" | sha256sum -c -
printf 'repository=unsloth/SmolLM2-135M-Instruct-GGUF\nrevision=%s\nsha256=%s\n' \
  "$fixture_revision" "$fixture_sha" > "$out/fixture.txt"
timeout 180s env -i PATH=/usr/bin:/bin "$bundle/llama-server" \
  -m "$work/smollm2.gguf" --host 127.0.0.1 --port 18080 \
  -c 4096 -t 4 -ngl 0 --parallel 1 > "$out/engine-smoke.log" 2>&1 &
engine_pid=$!
for ((attempt=0; attempt<60; attempt++)); do
  if curl --fail --silent --max-time 2 http://127.0.0.1:18080/health >/dev/null; then break; fi
  kill -0 "$engine_pid" || { cat "$out/engine-smoke.log" >&2; exit 1; }
  sleep 1
done
curl --fail --silent --show-error --max-time 2 http://127.0.0.1:18080/health > "$out/health.json"
curl --fail --silent --show-error --max-time 45 http://127.0.0.1:18080/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{"messages":[{"role":"user","content":"Say hello."}],"max_tokens":16,"temperature":0,"stream":false}' \
  > "$out/reply.json"
python3 - "$out/reply.json" <<'PY'
import json, sys
with open(sys.argv[1]) as stream:
    reply = json.load(stream)
text = reply["choices"][0]["message"]["content"]
assert isinstance(text, str) and text.strip(), "fixture produced no reply text"
print("PASS: hash-verified SmolLM2 load and non-empty chat reply")
PY
kill "$engine_pid"
wait "$engine_pid" || true
engine_pid=
echo "Verified engine artifact: $out/$archive"
