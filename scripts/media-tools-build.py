#!/usr/bin/env python3
"""Build the native FFmpeg pair used by managed Home, with corresponding source."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import tarfile
import tempfile
import urllib.request

SOURCES = {
    "ffmpeg-9.0.1.tar.xz": (
        "https://ffmpeg.org/releases/ffmpeg-9.0.1.tar.xz",
        "cf38e0e28c7e5605942c4a77755349b0145804a397af37eb1fb4c77cb237f635",
    ),
    "x264-b35605ace3ddf7c1a5d67a2eb553f034aef41d55.tar.bz2": (
        "https://code.videolan.org/videolan/x264/-/archive/b35605ace3ddf7c1a5d67a2eb553f034aef41d55/x264-b35605ace3ddf7c1a5d67a2eb553f034aef41d55.tar.bz2",
        "6eeb82934e69fd51e043bd8c5b0d152839638d1ce7aa4eea65a3fedcf83ff224",
    ),
}


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(command, cwd, env, capture=False):
    print("+ " + " ".join(map(str, command)), flush=True)
    return subprocess.run(command, cwd=cwd, env=env, check=True,
                          stdout=subprocess.PIPE if capture else None,
                          stderr=subprocess.STDOUT if capture else None,
                          text=True).stdout


def verify(output, expected):
    if output.is_symlink() or not output.is_dir():
        raise ValueError("Media tools output must be a regular directory")
    for path in output.rglob("*"):
        mode = path.lstat().st_mode
        if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
            raise ValueError("Media tools output contains a nonregular path")
    info = json.loads((output / "build-info.json").read_text())
    for key, value in expected.items():
        if info.get(key) != value:
            raise ValueError(f"Cached media tools differ at {key}; remove the obsolete build output before rebuilding")
    files = info.get("files", {})
    required = {"bin/ffmpeg", "bin/ffprobe", "BUILD.md", "licenses/FFmpeg-COPYING.GPLv2", "licenses/x264-COPYING",
                "sources/media-tools-build.py", "sources/build-media-tools.sh"}
    required.update("sources/" + name for name in SOURCES)
    if expected["platform"].startswith("linux-"):
        required.add("licenses/musl-COPYRIGHT")
    actual = {str(p.relative_to(output)) for p in output.rglob("*") if p.is_file()}
    if actual != required | {"build-info.json"} or set(files) != required:
        raise ValueError("Cached media tools file inventory differs")
    for name, (_, digest) in SOURCES.items():
        if sha(output / "sources" / name) != digest:
            raise ValueError(f"Cached source differs from pinned upstream bytes: {name}")
    for name, key in (("media-tools-build.py", "recipe_sha256"), ("build-media-tools.sh", "wrapper_sha256")):
        if sha(output / "sources" / name) != expected[key]:
            raise ValueError(f"Cached build recipe differs: {name}")
    if "musl" in expected and sha(output / "licenses/musl-COPYRIGHT") != expected["musl"]["license_sha256"]:
        raise ValueError("Cached musl notice differs from the selected toolchain")
    for name, record in files.items():
        if Path(name).is_absolute() or any(p in ("", ".", "..") for p in name.split("/")):
            raise ValueError("Unsafe media tools file record")
        p = output
        for part in name.split("/"):
            p = p / part
            if p.is_symlink():
                raise ValueError("Media tools cache contains a symlink")
        if sha(p) != record["sha256"] or p.stat().st_size != record["size"]:
            raise ValueError(f"Cached media tools changed: {name}")
    for name in ("ffmpeg", "ffprobe"):
        if not os.access(output / "bin" / name, os.X_OK):
            raise ValueError(f"Cached media tool is not executable: {name}")
    return info


def unpack(archive, destination):
    with tarfile.open(archive) as tar:
        for member in tar.getmembers():
            path = Path(member.name)
            if path.is_absolute() or ".." in path.parts or not (member.isfile() or member.isdir()):
                raise ValueError(f"Unsupported source archive member: {member.name}")
        tar.extractall(destination)
    roots = list(destination.iterdir())
    if len(roots) != 1 or not roots[0].is_dir():
        raise ValueError("Source archive must contain one root directory")
    return roots[0]


def check_linkage(binary, system, env):
    if system == "Darwin":
        output = run(["otool", "-L", str(binary)], binary.parent, env, True)
        libraries = [line.strip().split(" ")[0] for line in output.splitlines()[1:]]
        if not libraries or any(not p.startswith(("/usr/lib/", "/System/Library/")) for p in libraries):
            raise ValueError(f"Non-system dependency in {binary.name}: {libraries}")
    else:
        output = run(["readelf", "-l", "-d", str(binary)], binary.parent, env, True)
        if "INTERP" in output or "(NEEDED)" in output:
            raise ValueError(f"Linux media tool has dynamic dependencies: {binary.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    output = args.output.absolute()
    system, machine = platform.system(), platform.machine()
    target = {("Darwin", "arm64"): "darwin-arm64", ("Linux", "x86_64"): "linux-amd64",
              ("Linux", "aarch64"): "linux-arm64"}.get((system, machine))
    if target is None:
        raise ValueError("Build on native Apple silicon, Linux x86_64 or Linux ARM64")
    jobs = int(os.environ.get("CARGO_BUILD_JOBS", "4"))
    if jobs not in range(1, 5):
        raise ValueError("CARGO_BUILD_JOBS must be between 1 and 4")
    compiler = "clang" if system == "Darwin" else "musl-gcc"
    for tool in (compiler, "make", "pkg-config", "otool" if system == "Darwin" else "readelf"):
        if shutil.which(tool) is None:
            raise ValueError(f"Native media builder needs {tool}")
    # Preserve SIMD on ARM; x86 builds require NASM rather than silently slowing encoding.
    if machine == "x86_64" and shutil.which("nasm") is None:
        raise ValueError("The x86_64 media builder needs nasm")
    env = dict(os.environ)
    env.update(CC=compiler, SOURCE_DATE_EPOCH="0", LC_ALL="C", PKG_CONFIG_PATH="")
    if system == "Darwin":
        env["MACOSX_DEPLOYMENT_TARGET"] = "12.0"
    recipe = Path(__file__).resolve()
    wrapper = recipe.with_name("build-media-tools.sh")
    expected = {"schema": "elastos.media-tools-build/v1", "platform": target,
                "sources": {name: {"url": url, "sha256": digest} for name, (url, digest) in SOURCES.items()},
                "recipe_sha256": sha(recipe), "wrapper_sha256": sha(wrapper),
                "compiler": subprocess.check_output([compiler, "--version"], text=True).splitlines()[0]}
    musl_license = None
    if system == "Linux":
        musl_license = Path(os.environ.get("ELASTOS_MUSL_LICENSE", "/usr/share/doc/musl/copyright"))
        musl_version = os.environ.get("ELASTOS_MUSL_VERSION")
        if not musl_version and shutil.which("dpkg-query"):
            musl_version = subprocess.check_output(["dpkg-query", "-W", "-f=${Version}", "musl"], text=True).strip()
        if not musl_version or not musl_license.is_file() or musl_license.stat().st_size == 0:
            raise ValueError("Identify the builder's musl with ELASTOS_MUSL_VERSION and ELASTOS_MUSL_LICENSE")
        expected["musl"] = {"version": musl_version, "license_sha256": sha(musl_license)}
    if output.exists() or output.is_symlink():
        verify(output, expected)
        print(f"Reusing verified media tools: {output}")
        return
    output.parent.mkdir(parents=True, exist_ok=True)
    disk = shutil.disk_usage(output.parent)
    if disk.free * 10 < disk.total or disk.free < 4 * 1024**3:
        raise ValueError("Media build needs 4 GiB free and at least 10% free volume space")
    with tempfile.TemporaryDirectory(prefix=".media-tools-build-", dir=output.parent) as temporary:
        work = Path(temporary)
        package = work / "media-tools"
        for name in ("bin", "sources", "licenses"):
            (package / name).mkdir(parents=True, exist_ok=True)
        roots = {}
        for name, (url, digest) in SOURCES.items():
            archive = package / "sources" / name
            print(f"Downloading pinned source: {url}", flush=True)
            with urllib.request.urlopen(url, timeout=60) as response, archive.open("wb") as dest:
                shutil.copyfileobj(response, dest)
            if sha(archive) != digest:
                raise ValueError(f"Source checksum mismatch: {name}")
            dest = work / name.split("-")[0]
            dest.mkdir()
            roots[name.split("-")[0]] = unpack(archive, dest)
        prefix = work / "prefix"
        x264 = roots["x264"]
        run(["./configure", f"--prefix={prefix}", "--enable-static", "--disable-cli", "--disable-opencl",
             "--disable-avs", "--disable-lavf", "--disable-ffms", "--disable-gpac", "--disable-lsmash",
             "--disable-swscale"], x264, env)
        run(["make", f"-j{jobs}"], x264, env)
        run(["make", "install"], x264, env)
        env["PKG_CONFIG_LIBDIR"] = str(prefix / "lib/pkgconfig")
        ffmpeg = roots["ffmpeg"]
        flags = ["./configure", f"--prefix={work / 'install'}", f"--cc={compiler}",
                 "--disable-autodetect", "--disable-network", "--disable-shared", "--enable-static",
                 "--enable-gpl", "--enable-libx264", "--disable-ffplay", "--disable-doc",
                 "--disable-debug", "--pkg-config-flags=--static"]
        if system == "Linux":
            flags.append("--extra-ldexeflags=-static")
        run(flags, ffmpeg, env)
        run(["make", f"-j{jobs}", "ffmpeg", "ffprobe"], ffmpeg, env)
        for name in ("ffmpeg", "ffprobe"):
            binary = package / "bin" / name
            shutil.copy2(ffmpeg / name, binary)
            binary.chmod(0o755)
            check_linkage(binary, system, env)
            run([str(binary), "-version"], package, {"PATH": "/usr/bin:/bin"}, True)
        for src, name in ((ffmpeg / "COPYING.GPLv2", "FFmpeg-COPYING.GPLv2"), (x264 / "COPYING", "x264-COPYING")):
            shutil.copyfile(src, package / "licenses" / name)
        if musl_license is not None:
            shutil.copyfile(musl_license, package / "licenses/musl-COPYRIGHT")
        shutil.copyfile(recipe, package / "sources" / recipe.name)
        shutil.copyfile(wrapper, package / "sources" / wrapper.name)
        (package / "BUILD.md").write_text(
            "# ElastOS media tools\n\nFFmpeg 9.0.1 and x264 r3222 are built with GPLv2-enabled FFmpeg. "
            "Exact corresponding source and the unmodified build recipe are in sources/. "
            "Licenses are in licenses/. No nonfree or external network libraries are enabled.\n\n"
            "Build on native Apple silicon or Linux x86_64/ARM64 with Python 3, make and pkg-config. "
            "Mac requires Xcode command-line tools; Linux requires musl-gcc and binutils, plus NASM on x86_64. "
            "Linux also includes musl's copyright notices; build-info.json records its version. "
            "On non-Debian builders, set ELASTOS_MUSL_VERSION and ELASTOS_MUSL_LICENSE for that toolchain. "
            "Run `bash sources/build-media-tools.sh --output /absolute/path/to/new-media-tools`. "
            "The recipe verifies each source hash before building. Use CARGO_BUILD_JOBS=1..4.\n")
        info = dict(expected)
        info["files"] = {str(p.relative_to(package)): {"sha256": sha(p), "size": p.stat().st_size}
                         for p in sorted(package.rglob("*")) if p.is_file()}
        (package / "build-info.json").write_text(json.dumps(info, indent=2) + "\n")
        verify(package, expected)
        package.rename(output)
    print(f"Prepared native media tools: {output}")


if __name__ == "__main__":
    main()
