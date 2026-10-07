#!/usr/bin/env python3
"""Build one Cargo unit hermetically and key it by the closed-input rule.

Everything the build can see is staged first, byte for byte, into one work
directory; the key is the digest of those staged trees exactly as mounted,
so nothing can be visible without being keyed:

  /build/src        the exposed part of a git-archive checkout of HEAD: the
                    path packages of the unit's workspace resolve, their
                    declared edges (scripts/build-key-edges.json), workspace
                    manifest, lock and .cargo configs, rust-toolchain.toml and
                    the root manifests foreign path dependencies inherit from
  /build/cargo      a generated cargo home: an index config, one generated
                    index entry per locked registry package holding only its
                    locked line (which must carry the locked checksum), the
                    closure's .crate files verified against Cargo.lock and
                    unpacked from those archives, git dependencies as plain
                    archives of their locked revision (no .git anywhere)
  /build/toolchain  a copy of the pinned toolchain directory (rustc --print
                    sysroot) and nothing else of rustup
  /usr /etc ...     on Linux a sealed rootfs of the pinned image, never the
                    host (the seal is its content digest; /proc/cpuinfo is
                    masked); on macOS the system and Xcode, identified by
                    SDK/Xcode/clang versions
  /build/out        the only writable output; must not exist beforehand
  /build/home       a fresh empty HOME; /tmp a fresh tmpfs; no network

Every staged tree is canonicalized (all mtimes = SOURCE_DATE_EPOCH, modes as
git/tar recorded them) and digested over directories (incl. empty ones),
modes, sizes, mtimes, symlink targets and file contents. The environment is
cleared to an allowlist whose names and values are key fields; host-CPU
codegen flags (-C target-cpu/-C target-feature, -march/-mcpu/-mtune=native)
anywhere in it or in .cargo configs are refused; the effective target cfg
(rustc --print cfg with the allowed flags) is keyed. The sandbox runs as a
fixed uid/gid at canonical paths, so no embedded path or owner depends on
the host.

Key = sha256 of canonical JSON: {unit, src digest, cargo digest, toolchain
digest + rustc/cargo -vV, sysroot digest, env, target_cfg, sandbox}. It
needs no build (--key-only still stages, 2-5 s).

A producer builds twice in fresh sandboxes and records both final digests
and the host CPU model. One host cannot prove independence from its CPU, so
the receipt leaves `reusable` unset; `attest A.json B.json` from two hosts
with different CPU models and identical keys and digests is what marks a
unit reusable. Build scripts that read host CPU facts (cpu_detection) or
differing double builds mark it non-reusable outright. Exit 3 = miss.

  build-hermetic.py make-sysroot DIR            docker build/export/seal
  build-hermetic.py seal-sysroot DIR --image ID canonicalize + seal
  build-hermetic.py attest A.json B.json        cross-host reusability
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time

SPEC = importlib.util.spec_from_file_location("buildkey", Path(__file__).with_name("build-key.py"))
BK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BK)
Miss = BK.Miss

VERSION = 3
SOURCE_DATE_EPOCH = 1
SANDBOX_UID = 1000
PASS_THROUGH = set(BK.BUILD_ENV) | {"ELASTOS_RELEASE_VERSION", "SOURCE_DATE_EPOCH", "MACOSX_DEPLOYMENT_TARGET",
                                    "SDKROOT", "DEVELOPER_DIR", "RUSTUP_TOOLCHAIN", "CPPFLAGS"}
PASS_THROUGH_PREFIXES = BK.BUILD_ENV_PREFIXES + ("CPPFLAGS_", "HOST_CC", "HOST_CXX", "HOST_CFLAGS", "HOST_CXXFLAGS")
ROOT_FILES = ("Cargo.toml", "Cargo.lock", ".cargo/config.toml", ".cargo/config")
REPO_FILES = (".cargo/config.toml", ".cargo/config", "rust-toolchain.toml", "rust-toolchain")
SYSROOT_SEAL = ".hermetic-sysroot.json"
LINUX_PATHS = {"src": "/build/src", "out": "/build/out", "home": "/build/home", "cargo": "/build/cargo",
               "toolchain": "/build/toolchain"}
DARWIN_WORK = "/private/tmp/elastos-hermetic"

# Host-CPU-dependent codegen, anywhere it can hide.
CPU_FLAG = re.compile(r"(?:-C|--codegen)[\s=\"',]*(target-cpu|target-feature)=([^\s\"']+)|(-march)=([^\s\"']+)|"
                      r"(-mcpu|-mtune)=(native)")
CPU_FLAGS_ALLOWED = ()   # explicit (flag, value) pairs that may enter a reusable build; none today
CPU_DETECTION = re.compile(r"/proc/cpuinfo|is_x86_feature_detected|is_aarch64_feature_detected|__cpuid|"
                           r"\bcpuid\s*\(|getauxval|sysctlbyname|raw_cpuid|march=native|mcpu=native|target-cpu=native")


def sh(args, cwd=None, env=None, check=True):
    done = subprocess.run(args, cwd=cwd and str(cwd), env=env, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE)
    if check and done.returncode:
        raise Miss("%s failed (%d): %s" % (" ".join(map(str, args[:3])), done.returncode,
                                           done.stderr.strip()[-3000:]))
    return done


def sha256_path(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


# ---- canonical trees and digests -------------------------------------------

def canonicalize(root):
    """Make a staged tree host-independent: every mtime = SOURCE_DATE_EPOCH, directories 755, files 644
    or 755 (only the executable bit survives, as in git); symlinks untouched."""
    stamp = (SOURCE_DATE_EPOCH, SOURCE_DATE_EPOCH)
    for directory, dirnames, filenames in os.walk(str(root), topdown=False):
        for name in filenames + dirnames:
            path = os.path.join(directory, name)
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                continue
            if stat.S_ISDIR(info.st_mode):
                os.chmod(path, 0o755)
            elif stat.S_ISREG(info.st_mode):
                os.chmod(path, 0o755 if info.st_mode & 0o111 else 0o644)
            os.utime(path, stamp, follow_symlinks=False)
        os.chmod(directory, 0o755)
        os.utime(directory, stamp, follow_symlinks=False)


def tree_digest(root, skip=()):
    """Digest of a tree as mounted: every directory (incl. empty) with mode and mtime, every symlink with
    its target, every regular file with mode, size, mtime and sha256."""
    root = str(root)
    digest = hashlib.sha256()

    def note(line):
        digest.update(line.encode("utf-8", "surrogateescape") + b"\n")

    for directory, dirnames, filenames in os.walk(root):
        dirnames.sort()
        rel = os.path.relpath(directory, root)
        info = os.lstat(directory)
        # The root's own mtime changes whenever a seal is written next to its content.
        note("D %s %o %s" % (rel, stat.S_IMODE(info.st_mode), "-" if rel == "." else int(info.st_mtime)))
        for name in sorted(filenames + [d for d in dirnames if os.path.islink(os.path.join(directory, d))]):
            path = os.path.join(directory, name)
            entry = os.path.relpath(path, root)
            if entry in skip:
                continue
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                note("L %s -> %s" % (entry, os.readlink(path)))
            elif stat.S_ISREG(info.st_mode):
                note("F %s %o %d %d %s" % (entry, stat.S_IMODE(info.st_mode), info.st_size, int(info.st_mtime),
                                           sha256_path(path)))
            else:
                note("S %s %o" % (entry, info.st_mode))
    return digest.hexdigest()


def copy_tree(source, target, ignore=None):
    shutil.copytree(str(source), str(target), symlinks=True, ignore=ignore)


def rustup_bookkeeping(directory, names):
    """rustup's install records under lib/rustlib (component order, manifests, channel data): never
    read by the compiler, and the only files that differ between two installs of one toolchain."""
    if os.path.basename(directory) != "rustlib" or os.path.basename(os.path.dirname(directory)) != "lib":
        return set()
    return {n for n in names if n in ("components", "rust-installer-version", "install.log", "uninstall.sh")
            or n.startswith(("manifest-", "multirust-"))}


# ---- cargo metadata --------------------------------------------------------

def host_triple(env):
    for line in sh(["rustc", "-vV"], env=env).stdout.splitlines():
        if line.startswith("host: "):
            return line[len("host: "):].strip()
    raise Miss("rustc -vV reports no host")


def metadata(unit, manifest, env):
    args = ["cargo", "metadata", "--format-version", "1", "--offline", "--locked", "--manifest-path", str(manifest),
            "--filter-platform", unit.target or host_triple(env)]
    if unit.features:
        args += ["--features", ",".join(unit.features)]
    if unit.no_default_features:
        args += ["--no-default-features"]
    return json.loads(sh(args, env=env).stdout)


def closure_of(meta, unit):
    """Package ids the unit builds: deps of the root (dev-deps only for test units), build deps throughout."""
    packages = {p["id"]: p for p in meta["packages"]}
    members = set(meta["workspace_members"])
    roots = [i for i in members if packages[i]["name"] == unit.package]
    if len(roots) != 1:
        raise Miss("package %s is not a workspace member (%d matches)" % (unit.package, len(roots)))
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    closure = {}
    pending = [roots[0]]
    while pending:
        current = pending.pop()
        if current in closure:
            continue
        closure[current] = sorted(nodes[current].get("features", []))
        for dep in nodes[current]["deps"]:
            kinds = {k.get("kind") for k in dep.get("dep_kinds", [{"kind": None}])}
            if "dev" in kinds and len(kinds) == 1 and not (current == roots[0] and unit.kind == "test"):
                continue
            pending.append(dep["pkg"])
    return closure, packages


def normalized_id(package, root):
    if package["source"] is None:
        rel = BK.relative(Path(package["manifest_path"]).parent, root)
        return "path:%s#%s" % (rel, package["version"])
    return "%s#%s@%s" % (package["source"].split("#")[0], package["name"], package["version"])


def workspace_root_of(manifest_path, cache, src, environ):
    """Repository-relative root manifest of the workspace containing manifest_path."""
    directory = str(Path(manifest_path).parent)
    for known, root in cache.items():
        if directory == known or directory.startswith(known + "/"):
            return root
    done = sh(["cargo", "locate-project", "--workspace", "--offline", "--message-format", "plain",
               "--manifest-path", manifest_path], env=environ)
    root = BK.relative(done.stdout.strip(), src)
    if root is None:
        raise Miss("workspace root of %s lies outside the repository" % manifest_path)
    cache[str(Path(done.stdout.strip()).parent)] = root
    return root


def index_cache_path(name):
    """Sparse-index cache entry of a crate name (cargo's prefix scheme)."""
    if len(name) == 1:
        return "1/" + name
    if len(name) == 2:
        return "2/" + name
    if len(name) == 3:
        return "3/" + name[0] + "/" + name
    return name[:2] + "/" + name[2:4] + "/" + name


def index_entry_line(entry, version):
    """The index line of one version inside cargo's sparse-index cache file."""
    parts = entry.read_bytes().split(b"\0")
    # header: cache version, index version, last-updated; then (version, json) pairs
    for i in range(4, len(parts) - 1, 2):
        if parts[i] == version.encode():
            return parts[i + 1]
    return None


def index_cache_file(versions):
    """A cargo sparse-index cache file holding exactly the given (version, line) pairs."""
    body = b"".join(version.encode() + b"\0" + line + b"\0" for version, line in sorted(versions))
    return b"\x03" + (2).to_bytes(4, "little") + b"hermetic" + b"\0" + body


# ---- the plan ----------------------------------------------------------------

class Plan:
    """What the sandbox exposes, derived from the checkout and the host caches."""

    def __init__(self, unit, repo, commit, src, edges_rel, environ, host_cargo):
        self.unit = unit
        self.repo = repo
        self.commit = commit
        self.src = src
        self.environ = environ
        self.manifest = src / BK.relative(unit.manifest, repo)
        self.workspace = BK.relative(unit.manifest.parent, repo)
        meta = metadata(unit, self.manifest, environ)
        self.closure, packages = closure_of(meta, unit)
        edges_path = src / edges_rel
        self.edges, _ = BK.load_edges(edges_path)
        self.trees = set()
        self.files = set()
        for name in ROOT_FILES:
            if (src / self.workspace / name).is_file():
                self.files.add(self.workspace + "/" + name if self.workspace != "." else name)
        for name in REPO_FILES:
            if (src / name).is_file():
                self.files.add(name)
        if edges_path.is_file():
            self.files.add(edges_rel)
        roots = {}
        for package in packages.values():
            if package["source"] is None:
                directory = Path(package["manifest_path"]).parent
                rel = BK.relative(directory, src)
                if rel is None:
                    raise Miss("path package %s lies outside the repository" % package["id"])
                self.trees.add(rel)
                if directory != self.manifest.parent and not str(directory).startswith(str(self.manifest.parent) + "/"):
                    self.files.add(workspace_root_of(package["manifest_path"], roots, src, environ))
        self.edge_paths = set()
        for package_id in self.closure:
            self.edge_paths.update(self.edges["packages"].get(packages[package_id]["name"], []))
        unit_edges = self.edges["units"].get(unit.package + "/" + unit.kind, {})
        self.edge_paths.update(unit_edges.get("paths", []))
        self.edge_env = set(unit_edges.get("env", []))
        for rel in sorted(self.edge_paths):
            if not (src / rel).exists():
                raise Miss("declared edge %s is not in commit %s" % (rel, commit[:10]))
        entries = BK.parse_lock(self.manifest.parent / "Cargo.lock")
        self.lock = BK.lock_slice_of(entries, {normalized_id(packages[i], src): f for i, f in self.closure.items()})
        closure_crates = set()
        self.build_scripts = {}
        for package_id in self.closure:
            package = packages[package_id]
            source = package["source"]
            if source is not None and not source.startswith(("registry+", "git+")):
                raise Miss("unsupported package source %s" % package_id)
            if source is not None and source.startswith("registry+"):
                closure_crates.add(package["name"] + "-" + package["version"])
            for target in package.get("targets", []):
                if target.get("kind") == ["custom-build"] and source is None:
                    self.build_scripts[package["name"]] = Path(target["src_path"])
        self.registry = registry_state(entries, closure_crates, host_cargo)
        for name_version in self.registry["crates"]:
            build_script = host_cargo / "registry" / "src" / self.registry["index"] / name_version / "build.rs"
            if build_script.is_file():
                self.build_scripts[name_version] = build_script
        self.git = {}
        for entry in entries:
            source = entry.get("source", "")
            if source.startswith("git+"):
                if "#" not in source:
                    raise Miss("lock entry for %s has no resolved git revision" % entry.get("name"))
                self.git["%s@%s" % (entry["name"], entry["version"])] = source.rsplit("#", 1)[1]
        refuse_cpu_flags(self.allowlisted(), [src / rel for rel in self.files if ".cargo/" in rel])

    def exposed(self):
        return sorted(self.trees | self.files | self.edge_paths)

    def allowlisted(self):
        """Caller variables that enter the sandbox."""
        return {name: value for name, value in self.environ.items()
                if name in PASS_THROUGH or name.startswith(PASS_THROUGH_PREFIXES) or name in self.edge_env}

    def keyed_environment(self):
        values = self.allowlisted()
        values["ELASTOS_RELEASE_VERSION"] = self.environ.get("ELASTOS_RELEASE_VERSION") or "unversioned"
        for name in sorted(self.edge_env):
            values.setdefault(name, None)
        return values


def refuse_cpu_flags(env, config_files):
    sources = dict(env)
    for path in config_files:
        try:
            sources[str(path)] = path.read_text()
        except OSError:
            continue
    for origin, text in sorted(sources.items()):
        for match in CPU_FLAG.finditer(text or ""):
            flag = match.group(1) or match.group(3) or match.group(5)
            value = match.group(2) or match.group(4) or match.group(6)
            if (flag, value) not in CPU_FLAGS_ALLOWED:
                raise Miss("%s sets %s=%s; host-CPU codegen flags are not allowed in hermetic builds"
                           % (origin, flag, value))


def rustflags_tokens(env):
    if env.get("CARGO_ENCODED_RUSTFLAGS"):
        return [t for t in env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if t]
    for name in ("RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"):
        if env.get(name):
            return env[name].split()
    return []


def cpu_detection(build_scripts):
    """Crates whose build script reads host CPU facts at build time."""
    found = []
    for name, path in sorted(build_scripts.items()):
        try:
            text = re.sub(r"//[^\n]*", "", path.read_text(errors="replace"))
        except OSError:
            continue
        if CPU_DETECTION.search(text):
            found.append(name)
    return found


# ---- staging -----------------------------------------------------------------

def registry_state(lock_entries, closure_crates, host_cargo):
    """Locate the host registry state the sandbox needs, verifying every piece against Cargo.lock."""
    registry = host_cargo / "registry"
    locked = [e for e in lock_entries if e.get("source", "").startswith("registry+")]
    state = {"index": None, "config": None, "lines": {}, "crates": {}}
    if not locked:
        return state
    indexes = set()
    for entry in locked:
        name_version = "%s-%s" % (entry["name"], entry["version"])
        found = sorted((registry / "cache").glob("*/" + name_version + ".crate")) if (registry / "cache").is_dir() else []
        if len(found) != 1:
            raise Miss("%s.crate: %d copies in the cargo cache; run cargo fetch" % (name_version, len(found)))
        indexes.add(found[0].parent.name)
        if name_version in closure_crates:
            actual = sha256_path(found[0])
            if actual != entry.get("checksum"):
                raise Miss("%s.crate sha256 %s does not match Cargo.lock %s" % (name_version, actual, entry.get("checksum")))
            state["crates"][name_version] = found[0]
    if len(indexes) != 1:
        raise Miss("locked registry packages come from %d indexes; one is supported" % len(indexes))
    state["index"] = indexes.pop()
    host_index = registry / "index" / state["index"]
    config = host_index / "config.json"
    if not config.is_file():
        raise Miss("registry index %s has no config.json" % state["index"])
    state["config"] = config.read_bytes()
    for entry in locked:
        cache = host_index / ".cache" / index_cache_path(entry["name"])
        line = index_entry_line(cache, entry["version"]) if cache.is_file() else None
        if line is None:
            raise Miss("index entry for %s %s is missing; run cargo fetch" % (entry["name"], entry["version"]))
        if ('"cksum":"%s"' % entry.get("checksum")).encode() not in line:
            raise Miss("index entry for %s %s does not carry the Cargo.lock checksum" % (entry["name"], entry["version"]))
        state["lines"][(entry["name"], entry["version"])] = line
    return state


def stage_source(plan, stage):
    """Only the exposed paths of the checkout."""
    for rel in plan.exposed():
        source = plan.src / rel
        target = stage / rel
        if target.exists():
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.is_dir():
            copy_tree(source, target)
        else:
            shutil.copy2(str(source), str(target), follow_symlinks=False)


def stage_cargo_home(plan, host_cargo, staging):
    """A cargo home generated from verified pieces only."""
    state = plan.registry
    if state["index"]:
        index_dir = staging / "registry" / "index" / state["index"]
        index_dir.mkdir(parents=True)
        (index_dir / "config.json").write_bytes(state["config"])
        by_name = {}
        for (name, version), line in state["lines"].items():
            by_name.setdefault(name, []).append((version, line))
        for name, versions in sorted(by_name.items()):
            entry = index_dir / ".cache" / index_cache_path(name)
            entry.parent.mkdir(parents=True, exist_ok=True)
            entry.write_bytes(index_cache_file(versions))
    for name_version, crate in sorted(state["crates"].items()):
        cache_dir = staging / "registry" / "cache" / state["index"]
        cache_dir.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(crate, cache_dir / crate.name)
        src_dir = staging / "registry" / "src" / state["index"]
        src_dir.mkdir(parents=True, exist_ok=True)
        with tarfile.open(crate) as archive:
            for member in archive.getmembers():
                if not member.name.startswith(name_version + "/") or ".." in member.name.split("/"):
                    raise Miss("%s.crate contains an unexpected path %s" % (name_version, member.name))
                if member.issym() or member.islnk() or member.isdev():
                    raise Miss("%s.crate contains a link or device %s" % (name_version, member.name))
            archive.extractall(src_dir)
        (src_dir / name_version / ".cargo-ok").write_text('{"v":1}')
    for package_id, revision in sorted(plan.git.items()):
        stage_git_dependency(host_cargo, staging, package_id, revision)


def stage_git_dependency(host_cargo, staging, package_id, revision):
    """The locked revision as a plain archive (no .git) plus the bare database cargo resolves against."""
    checkouts = host_cargo / "git" / "checkouts"
    for checkout in sorted(checkouts.glob("*/*")) if checkouts.is_dir() else []:
        head = sh(["git", "rev-parse", "HEAD"], cwd=checkout, check=False)
        if head.returncode or head.stdout.strip() != revision:
            continue
        db = host_cargo / "git" / "db" / checkout.parent.name
        if not db.is_dir():
            raise Miss("git database for %s is missing" % package_id)
        if sh(["git", "cat-file", "-e", revision + "^{commit}"], cwd=db, check=False).returncode:
            raise Miss("git database for %s lacks revision %s" % (package_id, revision))
        target = staging / "git" / "checkouts" / checkout.parent.name / checkout.name
        target.mkdir(parents=True)
        archive = subprocess.Popen(["git", "archive", "--format=tar", revision], cwd=str(db), stdout=subprocess.PIPE)
        extract = subprocess.run(["tar", "-x", "-C", str(target)], stdin=archive.stdout)
        archive.stdout.close()
        if archive.wait() or extract.returncode:
            raise Miss("git archive of %s at %s failed" % (package_id, revision))
        (target / ".cargo-ok").write_text("ok")
        # cargo resolves the locked revision through the database; a bare
        # clone of just that revision carries no host remote configuration.
        staged_db = staging / "git" / "db" / checkout.parent.name
        sh(["git", "clone", "--quiet", "--bare", "--no-hardlinks", str(db), str(staged_db)])
        sh(["git", "-C", str(staged_db), "config", "--unset-all", "remote.origin.url"], check=False)
        return
    raise Miss("no checkout of %s at %s in the cargo git cache; run cargo fetch" % (package_id, revision))


# ---- sysroot -----------------------------------------------------------------

def seal_sysroot(directory, image):
    directory = Path(directory).resolve()
    canonicalize(directory)
    seal = {"image": image, "digest": tree_digest(directory, skip=(SYSROOT_SEAL,))}
    (directory / SYSROOT_SEAL).write_text(json.dumps(seal, indent=1, sort_keys=True) + "\n")
    return seal


def make_sysroot(directory, dockerfile):
    """Build the pinned image with docker, export its rootfs into directory and seal it."""
    directory = Path(directory).resolve()
    if directory.exists():
        raise Miss("%s exists; make-sysroot needs a new directory" % directory)
    tag = "elastos-hermetic-sysroot:local"
    sh(["docker", "build", "-q", "-t", tag, "-f", str(dockerfile), str(Path(dockerfile).parent)])
    image = sh(["docker", "image", "inspect", "--format", "{{.Id}}", tag]).stdout.strip()
    container = sh(["docker", "create", tag]).stdout.strip()
    directory.mkdir(parents=True)
    try:
        export = subprocess.Popen(["docker", "export", container], stdout=subprocess.PIPE)
        extract = subprocess.run(["tar", "-x", "--no-same-owner", "--no-same-permissions", "--exclude=dev/*",
                                  "-C", str(directory)], stdin=export.stdout)
        export.stdout.close()
        if export.wait() or extract.returncode:
            raise Miss("docker export of %s failed" % tag)
    finally:
        sh(["docker", "rm", container], check=False)
    return seal_sysroot(directory, image)


def verified_sysroot(directory):
    directory = Path(directory).resolve()
    try:
        seal = json.loads((directory / SYSROOT_SEAL).read_text())
    except (OSError, ValueError):
        raise Miss("%s is not a sealed sysroot (run seal-sysroot)" % directory)
    started = time.time()
    actual = tree_digest(directory, skip=(SYSROOT_SEAL,))
    if actual != seal.get("digest"):
        raise Miss("sysroot %s drifted from its seal (%s != %s)" % (directory, actual[:12], str(seal.get("digest"))[:12]))
    return directory, {"image": seal.get("image"), "digest": actual}, time.time() - started


def darwin_identity():
    first = BK._first_line
    return {"xcrun-sdk-version": first(["xcrun", "--show-sdk-version"]),
            "xcrun-sdk-build": first(["xcrun", "--show-sdk-build-version"]),
            "xcode": first(["xcodebuild", "-version"]), "clang": first(["clang", "--version"])}


def host_cpu():
    """Model, stepping and feature flags: two hosts with the same string are the same CPU for attestation."""
    if sys.platform == "darwin":
        return BK._first_line(["sysctl", "-n", "machdep.cpu.brand_string"])
    fields = {}
    try:
        for line in Path("/proc/cpuinfo").read_text().splitlines():
            key, _, value = line.partition(":")
            key = key.strip()
            if key in ("model name", "stepping", "flags", "Features", "CPU part") and key not in fields:
                fields[key] = value.strip()
    except OSError:
        return platform.processor() or None
    if not fields:
        return platform.processor() or None
    flags = fields.get("flags") or fields.get("Features") or ""
    return "%s stepping %s flags %s" % (fields.get("model name") or fields.get("CPU part") or "?",
                                         fields.get("stepping", "?"), hashlib.sha256(flags.encode()).hexdigest()[:12])


# ---- sandboxes ---------------------------------------------------------------

class Bubblewrap:
    kind = "bwrap"
    SYSTEM = ("usr", "etc", "lib", "lib64", "bin", "sbin", "opt")

    def __init__(self, stage, out, sysroot):
        self.stage = stage
        self.out = out
        self.sysroot = sysroot
        self.paths = dict(LINUX_PATHS)

    def identity(self):
        return {"kind": self.kind, "paths": self.paths, "uid": SANDBOX_UID, "gid": SANDBOX_UID,
                "writable": ["<out>", "<home>", "/tmp"], "network": False, "cpuinfo": "masked"}

    def environment(self, allowlisted):
        env = {"PATH": "/build/toolchain/bin:/usr/local/bin:/usr/bin:/bin", "HOME": "/build/home", "TMPDIR": "/tmp",
               "CARGO_HOME": "/build/cargo", "CARGO_TARGET_DIR": "/build/out",
               "CARGO_BUILD_BUILD_DIR": "/build/out/build", "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never",
               "SOURCE_DATE_EPOCH": str(SOURCE_DATE_EPOCH)}
        env.update(allowlisted)
        return env

    def wrap(self, command, workdir):
        cpuinfo = self.stage / "cpuinfo"
        if not cpuinfo.is_file():
            cpuinfo.write_text("processor\t: 0\nflags\t\t:\n")
        args = ["bwrap", "--unshare-all", "--uid", str(SANDBOX_UID), "--gid", str(SANDBOX_UID), "--die-with-parent",
                "--new-session", "--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp",
                "--ro-bind", str(cpuinfo), "/proc/cpuinfo"]
        for top in self.SYSTEM:
            path = self.sysroot[0] / top
            if os.path.islink(path):
                args += ["--symlink", os.readlink(path), "/" + top]
            elif path.is_dir():
                args += ["--ro-bind", str(path), "/" + top]
        args += ["--ro-bind", str(self.stage / "toolchain"), "/build/toolchain",
                 "--bind", str(self.stage / "cargo"), "/build/cargo",
                 "--ro-bind", str(self.stage / "src"), "/build/src",
                 "--bind", str(self.out), "/build/out", "--tmpfs", "/build/home",
                 "--chdir", workdir, "--"]
        return args + command


class SandboxExec:
    """macOS: paths cannot be remounted, so canonical paths are the fixed staging paths."""
    kind = "sandbox-exec"
    SYSTEM = ("/usr", "/bin", "/sbin", "/System", "/Library", "/private/etc", "/private/var/db",
              "/private/var/select", "/Applications/Xcode.app", "/dev")

    def __init__(self, stage, out, sysroot):
        self.stage = stage
        self.out = out
        self.paths = {"src": str(stage / "src"), "out": str(out), "home": str(stage / "home"),
                      "cargo": str(stage / "cargo"), "toolchain": str(stage / "toolchain")}

    def identity(self):
        return {"kind": self.kind, "paths": self.paths, "writable": ["<out>", "<home>"], "network": False}

    def environment(self, allowlisted):
        env = {"PATH": "%s/bin:/usr/bin:/bin" % self.paths["toolchain"], "HOME": self.paths["home"],
               "TMPDIR": str(self.stage / "tmp"), "CARGO_HOME": self.paths["cargo"],
               "CARGO_TARGET_DIR": str(self.out), "CARGO_BUILD_BUILD_DIR": str(self.out / "build"),
               "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never", "SOURCE_DATE_EPOCH": str(SOURCE_DATE_EPOCH)}
        env.update(allowlisted)
        return env

    def profile(self):
        reads = [p for p in self.SYSTEM if os.path.exists(p)]
        reads += [str(self.stage / name) for name in ("toolchain", "cargo", "src", "home", "tmp")] + [str(self.out)]
        ancestors = set()
        for path in reads:
            ancestors.update(str(parent) for parent in Path(path).parents)
        lines = ["(version 1)", "(deny default)", "(allow process*)", "(allow sysctl-read)", "(allow mach-lookup)",
                 "(allow ipc-posix*)", "(allow signal)", "(allow file-read-metadata)", "(allow file-ioctl)",
                 "(allow file-read*" + "".join('\n  (literal "%s")' % p for p in sorted(ancestors))
                 + "".join('\n  (subpath "%s")' % p for p in reads) + ")",
                 '(allow file-write* (subpath "%s") (subpath "%s") (subpath "%s") (subpath "%s")'
                 ' (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))'
                 % (self.out, self.stage / "home", self.stage / "tmp", self.stage / "cargo"),
                 "(deny network*)", ""]
        return "\n".join(lines)

    def wrap(self, command, workdir):
        return ["sandbox-exec", "-p", self.profile()] + command


# ---- producer ----------------------------------------------------------------

def unit_slug(unit):
    return "-".join(filter(None, [unit.package, unit.kind, unit.name if unit.kind == "bin" else None,
                                  unit.profile, unit.target]))


class Producer:
    def __init__(self, options, environ):
        self.options = options
        self.unit = BK.Unit(options.manifest_path, options.package, options.kind, options.name, options.target,
                            options.profile, [f for f in options.features.split(",") if f],
                            options.no_default_features)
        self.repo = BK.repo_root(self.unit.manifest.parent)
        if sh(["git", "status", "--porcelain"], cwd=self.repo).stdout.strip() and not options.allow_dirty:
            raise Miss("worktree has uncommitted changes; a hermetic build keys HEAD exactly (commit, or --allow-dirty)")
        self.commit = sh(["git", "rev-parse", "HEAD"], cwd=self.repo).stdout.strip()
        self.temporary = options.work is None and sys.platform != "darwin"
        default_work = DARWIN_WORK if sys.platform == "darwin" else tempfile.mkdtemp(prefix="hermetic-")
        self.work = Path(os.path.realpath(options.work or default_work))
        self.host_cargo = BK.cargo_home(environ)
        rustup_home = Path(os.path.realpath(environ.get("RUSTUP_HOME") or Path.home() / ".rustup"))
        # Host-side cargo/rustc calls must not let the caller's HOME pick another toolchain or cache.
        self.environ = dict(environ, RUSTUP_HOME=str(rustup_home), CARGO_HOME=str(self.host_cargo))
        self.toolchain = Path(os.path.realpath(sh(["rustc", "--print", "sysroot"], cwd=self.unit.manifest.parent,
                                                   env=self.environ).stdout.strip()))
        if not (self.toolchain / "bin" / "cargo").is_file() or not (self.toolchain / "bin" / "rustc").is_file():
            raise Miss("toolchain %s has no bin/cargo and bin/rustc" % self.toolchain)
        self.sysroot = None
        self.sysroot_seconds = 0.0
        if sys.platform != "darwin":
            if not options.sysroot:
                raise Miss("Linux hermetic builds need --sysroot <sealed image rootfs>")
            directory, identity, self.sysroot_seconds = verified_sysroot(options.sysroot)
            self.sysroot = (directory, identity)
            if not shutil.which("bwrap"):
                raise Miss("bwrap (bubblewrap) not found")
        elif not shutil.which("sandbox-exec"):
            raise Miss("sandbox-exec not found")

    def prepare(self):
        """Fresh checkout, staged trees and their digests for one build, always at the same paths."""
        work = self.work / "current"
        if work.exists():
            shutil.rmtree(work)
        src = work / "checkout"
        src.mkdir(parents=True)
        archive = subprocess.Popen(["git", "archive", "--format=tar", self.commit], cwd=str(self.repo), stdout=subprocess.PIPE)
        extract = subprocess.run(["tar", "-x", "-C", str(src)], stdin=archive.stdout)
        archive.stdout.close()
        if archive.wait() or extract.returncode:
            raise Miss("git archive of %s failed" % self.commit[:10])
        manifest = src / BK.relative(self.unit.manifest, self.repo)
        # The only networked step: fill the host cargo cache for the lock.
        sh(["cargo", "fetch", "--locked", "--manifest-path", str(manifest)], env=self.environ)
        plan = Plan(self.unit, self.repo, self.commit, src, self.options.edges, self.environ, self.host_cargo)
        stage = work / "stage"
        started = time.time()
        stage_source(plan, stage / "src")
        (stage / "cargo").mkdir(parents=True)
        stage_cargo_home(plan, self.host_cargo, stage / "cargo")
        copy_tree(self.toolchain, stage / "toolchain", ignore=rustup_bookkeeping)
        for name in ("home", "tmp"):
            (stage / name).mkdir()
        for name in ("src", "cargo", "toolchain"):
            canonicalize(stage / name)
        digests = {name: tree_digest(stage / name) for name in ("src", "cargo", "toolchain")}
        seconds = time.time() - started
        return plan, stage, digests, seconds

    def sandbox(self, stage, out):
        cls = SandboxExec if sys.platform == "darwin" else Bubblewrap
        return cls(stage, out, self.sysroot)

    def workdir(self, plan, sandbox):
        return sandbox.paths["src"] + "/" + plan.workspace if plan.workspace != "." else sandbox.paths["src"]

    def document(self, plan, stage, digests, sandbox):
        toolchain_bin = stage / "toolchain" / "bin"
        toolchain = {"name": self.toolchain.name, "digest": digests["toolchain"],
                     "rustc": sh([str(toolchain_bin / "rustc"), "-vV"], env=self.environ).stdout.strip(),
                     "cargo": sh([str(toolchain_bin / "cargo"), "-V"], env=self.environ).stdout.strip()}
        args = [str(toolchain_bin / "rustc"), "--print", "cfg"]
        if self.unit.target:
            args += ["--target", self.unit.target]
        args += rustflags_tokens(plan.allowlisted())
        target_cfg = sorted(line for line in sh(args, env=self.environ).stdout.splitlines() if line.strip())
        return {"version": VERSION, "unit": self.unit.describe(self.repo), "src": digests["src"],
                "cargo": digests["cargo"], "toolchain": toolchain,
                "sysroot": self.sysroot[1] if self.sysroot else darwin_identity(),
                "env": plan.keyed_environment(), "target_cfg": target_cfg, "sandbox": sandbox.identity()}

    def build(self, plan, stage, sandbox, out):
        if out.exists():
            raise Miss("output dir %s exists; hermetic builds start from an empty output dir" % out)
        out.mkdir(parents=True)
        command = self.unit.cargo_args()
        command[command.index("--manifest-path") + 1] = self.workdir(plan, sandbox) + "/Cargo.toml"
        command += ["--offline", "--locked"]
        started = time.time()
        done = subprocess.run(sandbox.wrap(command, self.workdir(plan, sandbox)), cwd=str(self.repo),
                              env=sandbox.environment(plan.allowlisted()), text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        seconds = time.time() - started
        if done.returncode:
            raise Miss("hermetic build failed (%d) after %.0fs: %s" % (done.returncode, seconds,
                                                                         done.stderr.strip()[-3000:]))
        outputs, final = {}, {}
        for line in done.stdout.splitlines():
            if not line.startswith("{"):
                continue
            message = json.loads(line)
            if message.get("reason") != "compiler-artifact":
                continue
            if BK.parse_package_id(message["package_id"])[1] != self.unit.package:
                continue
            names = ([message["executable"]] if message.get("executable") else []) + message.get("filenames", [])
            for name in names:
                host = str(out) + name[len(sandbox.paths["out"]):] if name.startswith(sandbox.paths["out"]) else name
                rel = name[len(sandbox.paths["out"]) + 1:] if name.startswith(sandbox.paths["out"]) else name
                outputs[rel] = BK.sha256_file(host)
                if name == message.get("executable") or not rel.startswith("build/"):
                    final[rel] = outputs[rel]
        if not outputs:
            raise Miss("the build produced no artifact for %s" % self.unit.package)
        return {"outputs": outputs, "final": final or outputs, "seconds": round(seconds, 1)}

    def diagnose(self, plan, sandbox, out):
        """Run the compiler's dep-info discovery inside the sandbox; flag reads outside the staged trees."""
        command = self.unit.cargo_args()
        command[command.index("--manifest-path") + 1] = self.workdir(plan, sandbox) + "/Cargo.toml"
        command += ["--offline", "--locked"]
        done = subprocess.run(sandbox.wrap(command, self.workdir(plan, sandbox)), cwd=str(self.repo),
                              env=sandbox.environment(plan.allowlisted()), text=True,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if done.returncode:
            raise Miss("diagnostic build failed: %s" % done.stderr[-2000:])
        paths = sandbox.paths
        to_host = lambda p: str(out) + p[len(paths["out"]):] if p.startswith(paths["out"]) else p
        artifacts = [json.loads(l) for l in done.stdout.splitlines() if l.startswith("{")]
        artifacts = [m for m in artifacts if m.get("reason") == "compiler-artifact"]
        deps_dirs = sorted({str(Path(to_host(f)).parent) for m in artifacts for f in m.get("filenames", [])
                            if Path(f).parent.name == "deps"})
        reads, outside, generated, env = set(), set(), 0, set()
        for message in artifacts:
            message = dict(message, filenames=[to_host(f) for f in message.get("filenames", [])],
                           executable=to_host(message["executable"]) if message.get("executable") else None)
            files, names = BK.parse_dep_info(BK.dep_info_for_artifact(message, deps_dirs))
            env.update(names)
            for name in files:
                name = os.path.normpath(os.path.join(self.workdir(plan, sandbox), name))
                if name.startswith(paths["src"] + "/"):
                    reads.add(name[len(paths["src"]) + 1:])
                elif name.startswith(paths["out"] + "/"):
                    generated += 1
                elif not name.startswith((paths["cargo"] + "/", paths["toolchain"] + "/")):
                    outside.add(name)
        print("diagnose: %d repository files read, %d generated, %d env names" % (len(reads), generated, len(env)))
        if outside:
            raise Miss("compiler read outside the staged trees: " + ", ".join(sorted(outside)))


def produce(options, environ):
    """Key the unit; unless --key-only, build it twice in fresh sandboxes at identical paths."""
    producer = Producer(options, environ)
    base_out = Path(os.path.realpath(options.out)) if options.out else producer.repo / "target-hermetic" / unit_slug(producer.unit)
    if base_out.exists() and not options.key_only:
        raise Miss("output dir %s exists; hermetic builds start from an empty output dir" % base_out)
    current = base_out / "current"
    plan, stage, digests, staging_seconds = producer.prepare()
    sandbox = producer.sandbox(stage, current)
    document = producer.document(plan, stage, digests, sandbox)
    key = hashlib.sha256(BK.canonical(document).encode("utf-8")).hexdigest()
    receipt = {"key": key, "commit": producer.commit, "unit": document["unit"], "document": document,
               "host": {"cpu": host_cpu(), "platform": platform.platform()},
               "staging_seconds": round(staging_seconds, 1), "sysroot_verify_seconds": round(producer.sysroot_seconds, 1),
               "cpu_detection": cpu_detection(plan.build_scripts), "reusable": None}
    if options.key_only:
        return receipt, producer
    builds = [producer.build(plan, stage, sandbox, current)]
    if options.diagnose:
        producer.diagnose(plan, sandbox, current)
    current.rename(base_out / "a")
    if not options.single:
        plan_b, stage_b, digests_b, _ = producer.prepare()
        sandbox_b = producer.sandbox(stage_b, current)
        key_b = hashlib.sha256(BK.canonical(producer.document(plan_b, stage_b, digests_b, sandbox_b)).encode()).hexdigest()
        if key_b != key:
            raise Miss("the second preparation keyed differently (%s != %s)" % (key_b[:12], key[:12]))
        builds.append(producer.build(plan_b, stage_b, sandbox_b, current))
        current.rename(base_out / "b")
        if builds[0]["final"] != builds[1]["final"]:
            receipt["reusable"], receipt["reason"] = False, "two builds on this host differ"
        elif receipt["cpu_detection"]:
            receipt["reusable"], receipt["reason"] = False, "build scripts read host CPU facts: " + ", ".join(receipt["cpu_detection"])
        else:
            receipt["reason"] = "identical on one host; attest with a receipt from a host with another CPU"
    else:
        receipt["reusable"], receipt["reason"] = False, "single build"
    receipt["builds"] = builds
    receipt["out"] = str(base_out)
    return receipt, producer


def attest(receipts):
    """Two producers on different CPUs agreeing byte for byte make a unit reusable."""
    loaded = [json.loads(Path(p).read_text()) for p in receipts]
    keys = {r["key"] for r in loaded}
    if len(loaded) < 2:
        return False, "attestation needs receipts from two hosts"
    if len(keys) != 1:
        return False, "keys differ: " + ", ".join(sorted(k[:12] for k in keys))
    if any(r.get("reusable") is False for r in loaded):
        return False, "a producer refused reuse: " + "; ".join(r.get("reason", "") for r in loaded if r.get("reusable") is False)
    finals = [b["final"] for r in loaded for b in r.get("builds", [])]
    if len(finals) < 4 or any(f != finals[0] for f in finals):
        return False, "final artifacts differ across %d builds" % len(finals)
    cpus = [r.get("host", {}).get("cpu") for r in loaded]
    if len(set(cpus)) < 2 or None in cpus:
        return False, "producers report the same CPU model (%s); a second CPU is required" % cpus[0]
    return True, "reusable: %d builds on %s agree" % (len(finals), " and ".join(sorted(set(cpus))))


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")
    seal = sub.add_parser("seal-sysroot", help="canonicalize a sysroot rootfs and record its content digest")
    seal.add_argument("directory")
    seal.add_argument("--image", required=True, help="container image id or digest the rootfs was exported from")
    make = sub.add_parser("make-sysroot", help="docker build + export + seal the pinned sysroot image")
    make.add_argument("directory")
    make.add_argument("--dockerfile", default=str(Path(__file__).with_name("hermetic-sysroot.Dockerfile")))
    att = sub.add_parser("attest", help="combine receipts from two hosts into a reusability verdict")
    att.add_argument("receipts", nargs="+")
    parser.add_argument("--manifest-path", help="workspace root Cargo.toml of the unit")
    parser.add_argument("--package")
    parser.add_argument("--kind", choices=BK.KINDS)
    parser.add_argument("--name", help="binary name (bin units; default: package name)")
    parser.add_argument("--target", help="target triple (default: host)")
    parser.add_argument("--profile", help="cargo profile (default: dev, or test for test units)")
    parser.add_argument("--features", default="", help="comma-separated features")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--edges", default="scripts/build-key-edges.json", help="edges map, repository-relative")
    parser.add_argument("--sysroot", help="Linux: sealed rootfs of the pinned build image")
    parser.add_argument("--out", help="output dir (must not exist; default: <repo>/target-hermetic/<unit>)")
    parser.add_argument("--work", help="host dir for staging (default: temp; macOS: %s)" % DARWIN_WORK)
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument("--allow-dirty", action="store_true", help="key HEAD even if the worktree is dirty")
    parser.add_argument("--key-only", action="store_true", help="stage and key without building")
    parser.add_argument("--single", action="store_true", help="build once; the receipt marks the unit non-reusable")
    parser.add_argument("--diagnose", action="store_true", help="after building, list dep-info reads")
    parser.add_argument("--receipt", help="write key, document, both builds' digests and host CPU as JSON")
    parser.add_argument("--json", help="write the canonical key document")
    options = parser.parse_args(argv)
    if options.command == "seal-sysroot":
        sealed = seal_sysroot(options.directory, options.image)
        print("sealed %s image=%s digest=%s" % (options.directory, sealed["image"], sealed["digest"]))
        return 0
    if options.command == "make-sysroot":
        try:
            sealed = make_sysroot(options.directory, options.dockerfile)
        except Miss as miss:
            print("make-sysroot failed: %s" % miss)
            return 1
        print("sysroot %s image=%s digest=%s" % (options.directory, sealed["image"], sealed["digest"]))
        return 0
    if options.command == "attest":
        reusable, reason = attest(options.receipts)
        print("reusable=%s %s" % (reusable, reason))
        return 0 if reusable else BK.EXIT_MISS
    if not (options.manifest_path and options.package and options.kind):
        parser.error("--manifest-path, --package and --kind are required")
    producer = None
    try:
        receipt, producer = produce(options, dict(os.environ))
        document = receipt["document"]
        if options.json:
            Path(options.json).write_text(BK.canonical(document) + "\n")
        summary = "key=%s src=%s cargo=%s toolchain=%s sandbox=%s cpu=%s" % (
            receipt["key"], document["src"][:12], document["cargo"][:12], document["toolchain"]["digest"][:12],
            document["sandbox"]["kind"], receipt["host"]["cpu"])
        if options.key_only:
            print(summary)
        else:
            builds = receipt["builds"]
            print(summary + " reusable=%s seconds=%s (%s)" % (
                receipt["reusable"], "+".join("%.0f" % b["seconds"] for b in builds), receipt["reason"]))
            for index, build in enumerate(builds):
                for rel, digest in sorted(build["final"].items()):
                    print("build %d %s %s" % (index + 1, digest, rel))
        if options.receipt:
            Path(options.receipt).write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
    except Miss as miss:
        print("key=None reason=%s" % miss)
        return BK.EXIT_MISS
    finally:
        if producer is not None and producer.temporary and not options.keep_work:
            shutil.rmtree(producer.work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
