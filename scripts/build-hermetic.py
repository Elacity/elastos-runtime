#!/usr/bin/env python3
"""Build one Cargo unit hermetically and key it by construction.

The unit (package, kind bin|lib|test, target triple, profile, features,
version layer) is built from a clean checkout of HEAD (git archive: no
untracked or ignored file exists) inside a sandbox whose whole world is:

  /build/src        the exposed part of the checkout, read-only: the path
                    packages of the unit's workspace resolve, their declared
                    edges (scripts/build-key-edges.json), workspace manifest,
                    lock and .cargo configs, rust-toolchain.toml, and the
                    root manifests of foreign workspaces that path
                    dependencies inherit from
  /build/cargo      a cargo home staged for this build only: the index
                    config and the index entries of the lock's packages
                    (each must carry the locked checksum), the closure's
                    .crate files each verified against its Cargo.lock sha256
                    and unpacked from that archive, git dependencies at their
                    locked revision (clean checkouts verified with git)
  /build/toolchain  only the pinned toolchain directory (rustc --print
                    sysroot), read-only, keyed by a content digest of every
                    file, mode and symlink
  /usr /etc ...     on Linux a sealed rootfs of the pinned image, never the
                    host: its content digest is verified every run and keyed;
                    /proc/cpuinfo is masked. On macOS the system and Xcode,
                    identified by SDK/Xcode/clang versions
  /build/out        the only writable output; must not exist beforehand
  /build/home       a fresh empty HOME; /tmp a fresh tmpfs; no network

The environment is cleared to an allowlist whose names and values are key
fields; -C target-cpu / -C target-feature anywhere (env or .cargo config)
are refused unless allowlisted, and the effective target cfg (rustc --print
cfg with the allowed flags) is keyed. Paths inside the sandbox are
canonical, so CARGO_MANIFEST_DIR, OUT_DIR, file!() and panic locations
never depend on where the host keeps its checkout. Anything else does not
exist for the build: an undeclared read fails it (a miss) or cannot
influence it.

Key = sha256 of canonical JSON: git tree ids of every exposed path package,
blob ids of the exposed root files and edges, the lock slice of the unit's
closure with resolved features, the staged index content, the allowlisted
environment, the toolchain digest, the target cfg, the edges map blob and
the sandbox identity (canonical paths, sysroot digest, exposed set). It
needs no build (--key-only).

A producer builds twice in two fresh sandboxes; the unit is reusable only
when both builds give byte-identical final artifacts and no build script in
the closure reads host CPU facts (cpu_detection in the receipt). Both
digests go into the receipt. Dep-info discovery stays available as a
diagnostic (--diagnose). Exit 3 = miss.

  build-hermetic.py make-sysroot DIR     docker build/export/seal the image
  build-hermetic.py seal-sysroot DIR --image ID
"""
import argparse
import hashlib
import importlib.util
import json
import os
import re
from pathlib import Path
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

VERSION = 2
PASS_THROUGH = set(BK.BUILD_ENV) | {"ELASTOS_RELEASE_VERSION", "SOURCE_DATE_EPOCH", "MACOSX_DEPLOYMENT_TARGET",
                                    "SDKROOT", "DEVELOPER_DIR", "RUSTUP_TOOLCHAIN"}
PASS_THROUGH_PREFIXES = BK.BUILD_ENV_PREFIXES
ROOT_FILES = ("Cargo.toml", "Cargo.lock", ".cargo/config.toml", ".cargo/config")
REPO_FILES = (".cargo/config.toml", ".cargo/config", "rust-toolchain.toml", "rust-toolchain")
SYSROOT_SEAL = ".hermetic-sysroot.json"
LINUX_PATHS = {"src": "/build/src", "out": "/build/out", "home": "/build/home", "cargo": "/build/cargo",
               "toolchain": "/build/toolchain"}
DARWIN_WORK = "/private/tmp/elastos-hermetic"


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


def git_id(root, commit, rel):
    """Blob or tree id of <rel> at <commit>; Miss when absent."""
    done = sh(["git", "rev-parse", "--verify", "-q", "%s:%s" % (commit, rel)], cwd=root, check=False)
    if done.returncode:
        raise Miss("%s is not in commit %s" % (rel, commit[:10]))
    return done.stdout.strip()


def clean_checkout(root, commit, dest):
    dest.mkdir(parents=True)
    archive = subprocess.Popen(["git", "archive", "--format=tar", commit], cwd=str(root), stdout=subprocess.PIPE)
    extract = subprocess.run(["tar", "-x", "-C", str(dest)], stdin=archive.stdout)
    archive.stdout.close()
    if archive.wait() or extract.returncode:
        raise Miss("git archive of %s failed" % commit[:10])


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


class Plan:
    """Everything the sandbox exposes and the key covers."""

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
        self.closure_ids = {normalized_id(packages[i], src): f for i, f in self.closure.items()}
        edges_path = src / edges_rel
        self.edges, self.edges_blob = BK.load_edges(edges_path)
        self.edges_rel = edges_rel if edges_path.is_file() else None
        self.trees = set()
        self.files = set()
        for name in ROOT_FILES:
            if (src / self.workspace / name).is_file():
                self.files.add(self.workspace + "/" + name if self.workspace != "." else name)
        for name in REPO_FILES:
            if (src / name).is_file():
                self.files.add(name)
        if self.edges_rel:
            self.files.add(self.edges_rel)
        roots = {}
        # Every path package of the workspace resolve is exposed (cargo loads
        # all member manifests and path dependencies) and keyed by tree id.
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
        self.lock = BK.lock_slice_of(entries, self.closure_ids)
        # The resolver validates the whole workspace lock offline: it needs
        # index entries for every registry package and the git database of
        # every git package; only the closure's .crate files are unpacked.
        closure_crates = set()
        self.build_scripts = {}   # package name -> build script source on the host
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
        self.registry_state = registry_state(entries, closure_crates, host_cargo)
        for name_version, crate in self.registry_state["crates"].items():
            build_script = host_cargo / "registry" / "src" / self.registry_state["index"] / name_version / "build.rs"
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
        """Repository-relative paths bound read-only into the sandbox."""
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

    def document(self, sandbox, toolchain):
        trees = {rel: git_id(self.repo, self.commit, rel) for rel in sorted(self.trees)}
        files = {rel: git_id(self.repo, self.commit, rel) for rel in sorted(self.files | self.edge_paths)}
        state = self.registry_state
        index = {"index": state["index"], "config": state["config"][1] if state["config"] else None,
                 "entries": state["lines"]}
        return {"version": VERSION, "unit": self.unit.describe(self.repo), "trees": trees, "files": files,
                "closure": self.closure_ids, "lock": self.lock, "index": index, "env": self.keyed_environment(),
                "toolchain": toolchain.identity(), "target_cfg": toolchain.target_cfg(self.unit, self.allowlisted()),
                "edges": self.edges_blob, "sandbox": sandbox.identity()}


# ---- CPU policy ------------------------------------------------------------

CPU_FLAG = re.compile(r"(?:-C|--codegen)[\s=\"',]*(target-cpu|target-feature)=([^\s\"']+)")
CPU_FLAGS_ALLOWED = ()   # explicit (flag, value) pairs that may enter a reusable build; none today
CPU_DETECTION = re.compile(r"/proc/cpuinfo|is_x86_feature_detected|is_aarch64_feature_detected|__cpuid|"
                           r"\bcpuid\s*\(|getauxval|sysctlbyname|raw_cpuid|march=native|mcpu=native|target-cpu=native")


def refuse_cpu_flags(env, config_files):
    """Host-CPU-dependent codegen flags make bytes depend on the producer; refuse them everywhere they can hide."""
    sources = {name: value for name, value in env.items()}
    for path in config_files:
        try:
            sources[str(path)] = path.read_text()
        except OSError:
            continue
    for origin, text in sorted(sources.items()):
        for flag, value in CPU_FLAG.findall(text or ""):
            if (flag, value) not in CPU_FLAGS_ALLOWED:
                raise Miss("%s sets -C %s=%s; host-CPU codegen flags are not allowed in hermetic builds" % (origin, flag, value))


def rustflags_tokens(env):
    if env.get("CARGO_ENCODED_RUSTFLAGS"):
        return [t for t in env["CARGO_ENCODED_RUSTFLAGS"].split("\x1f") if t]
    for name in ("RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"):
        if env.get(name):
            return env[name].split()
    return []


def cpu_detection(build_scripts):
    """Crates whose build script reads host CPU facts at build time: their bytes may follow the producer's CPU."""
    found = []
    for name, path in sorted(build_scripts.items()):
        try:
            text = re.sub(r"//[^\n]*", "", path.read_text(errors="replace"))
        except OSError:
            continue
        if CPU_DETECTION.search(text):
            found.append(name)
    return found


# ---- staged cargo home ----------------------------------------------------

def index_entry_line(entry, version):
    """The index line of one version inside cargo's sparse-index cache file."""
    parts = entry.read_bytes().split(b"\0")
    # header: cache version, index version, last-updated; then (version, json) pairs
    for i in range(4, len(parts) - 1, 2):
        if parts[i] == version.encode():
            return parts[i + 1]
    return None


def registry_state(lock_entries, closure_crates, host_cargo):
    """Locate the host registry state the sandbox needs, verifying every piece against Cargo.lock.

    Returns {"index": dir name, "config": (path, sha256), "entries": {name: path},
             "lines": {name@version: sha256 of its index line}, "crates": {name-version: path}}."""
    registry = host_cargo / "registry"
    locked = [e for e in lock_entries if e.get("source", "").startswith("registry+")]
    state = {"index": None, "config": None, "entries": {}, "lines": {}, "crates": {}}
    if not locked:
        return state
    indexes = set()
    for entry in locked:
        name_version = "%s-%s" % (entry["name"], entry["version"])
        found = sorted(p for p in (registry / "cache").glob("*/" + name_version + ".crate")) if (registry / "cache").is_dir() else []
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
    state["config"] = (config, sha256_path(config))
    for entry in locked:
        cache = host_index / ".cache" / index_cache_path(entry["name"])
        line = index_entry_line(cache, entry["version"]) if cache.is_file() else None
        if line is None:
            raise Miss("index entry for %s %s is missing; run cargo fetch" % (entry["name"], entry["version"]))
        if ('"cksum":"%s"' % entry.get("checksum")).encode() not in line:
            raise Miss("index entry for %s %s does not carry the Cargo.lock checksum" % (entry["name"], entry["version"]))
        state["entries"][entry["name"]] = cache
        state["lines"]["%s@%s" % (entry["name"], entry["version"])] = hashlib.sha256(line).hexdigest()
    return state


def stage_cargo_home(plan, host_cargo, staging):
    """Copy only verified, lock-relevant registry and git state into a fresh cargo home."""
    state = plan.registry_state
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
    if state["index"]:
        staged_index = staging / "registry" / "index" / state["index"]
        staged_index.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(state["config"][0], staged_index / "config.json")
        for name, entry in sorted(state["entries"].items()):
            target = staged_index / ".cache" / index_cache_path(name)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(entry, target)
    for package_id, revision in sorted(plan.git.items()):
        stage_git_checkout(host_cargo, staging, package_id, revision)


def stage_git_checkout(host_cargo, staging, package_id, revision):
    checkouts = host_cargo / "git" / "checkouts"
    for checkout in sorted(checkouts.glob("*/*")) if checkouts.is_dir() else []:
        head = sh(["git", "rev-parse", "HEAD"], cwd=checkout, check=False)
        if head.returncode or head.stdout.strip() != revision:
            continue
        status = [line for line in sh(["git", "status", "--porcelain", "--ignored"], cwd=checkout).stdout.splitlines()
                  if not line.endswith(".cargo-ok")]  # cargo's own extraction marker
        if status:
            raise Miss("git checkout %s of %s is modified:\n%s" % (checkout, package_id, "\n".join(status)[:500]))
        db = host_cargo / "git" / "db" / checkout.parent.name
        if not db.is_dir():
            raise Miss("git database for %s is missing" % package_id)
        shutil.copytree(checkout, staging / "git" / "checkouts" / checkout.parent.name / checkout.name, symlinks=True)
        shutil.copytree(db, staging / "git" / "db" / checkout.parent.name, symlinks=True)
        return
    raise Miss("no clean checkout of %s at %s in the cargo git cache; run cargo fetch" % (package_id, revision))


# ---- content digests -------------------------------------------------------

def tree_digest(root, skip=()):
    """Content digest of a directory tree: every directory (incl. empty ones) with its mode,
    every symlink with its target, every regular file with mode, size and sha256."""
    root = str(root)
    digest = hashlib.sha256()

    def note(line):
        digest.update(line.encode("utf-8", "surrogateescape") + b"\n")

    for directory, dirnames, filenames in os.walk(root):
        dirnames.sort()
        rel = os.path.relpath(directory, root)
        if rel != ".":
            note("D %s %o" % (rel, stat.S_IMODE(os.lstat(directory).st_mode)))
        for name in sorted(filenames + [d for d in dirnames if os.path.islink(os.path.join(directory, d))]):
            path = os.path.join(directory, name)
            entry = os.path.relpath(path, root)
            if entry in skip:
                continue
            info = os.lstat(path)
            if stat.S_ISLNK(info.st_mode):
                note("L %s -> %s" % (entry, os.readlink(path)))
            elif stat.S_ISREG(info.st_mode):
                note("F %s %o %d %s" % (entry, stat.S_IMODE(info.st_mode), info.st_size, sha256_path(path)))
            else:
                note("S %s %o" % (entry, info.st_mode))
    return digest.hexdigest()


# ---- sysroot ---------------------------------------------------------------

def seal_sysroot(directory, image):
    directory = Path(directory).resolve()
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
    """The rootfs must match its seal's content digest; the digest (not the name) enters the key."""
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


# ---- sandboxes -------------------------------------------------------------

class Toolchain:
    """The pinned toolchain directory (rustc --print sysroot), keyed by its full content."""

    def __init__(self, workspace, environ):
        self.directory = Path(os.path.realpath(sh(["rustc", "--print", "sysroot"], cwd=workspace, env=environ).stdout.strip()))
        if not (self.directory / "bin" / "cargo").is_file() or not (self.directory / "bin" / "rustc").is_file():
            raise Miss("toolchain %s has no bin/cargo and bin/rustc" % self.directory)
        self.environ = environ
        started = time.time()
        self.digest = tree_digest(self.directory)
        self.seconds = time.time() - started

    def identity(self):
        return {"name": self.directory.name, "digest": self.digest,
                "rustc": sh([str(self.directory / "bin" / "rustc"), "-vV"], env=self.environ).stdout.strip(),
                "cargo": sh([str(self.directory / "bin" / "cargo"), "-V"], env=self.environ).stdout.strip()}

    def target_cfg(self, unit, env):
        """Effective target configuration (arch, features, ...) the unit compiles with."""
        args = [str(self.directory / "bin" / "rustc"), "--print", "cfg"]
        if unit.target:
            args += ["--target", unit.target]
        args += rustflags_tokens(env)
        return sorted(line for line in sh(args, env=self.environ).stdout.splitlines() if line.strip())


class Bubblewrap:
    kind = "bwrap"
    SYSTEM = ("usr", "etc", "lib", "lib64", "bin", "sbin", "opt")

    def __init__(self, plan, work, out, toolchain, sysroot):
        self.plan = plan
        self.work = work
        self.out = out
        self.toolchain = toolchain
        self.sysroot = sysroot
        self.paths = dict(LINUX_PATHS)

    def identity(self):
        return {"kind": self.kind, "paths": self.paths, "sysroot": self.sysroot[1],
                "exposed": self.plan.exposed(), "writable": ["<out>", "<home>", "/tmp"], "network": False,
                "cpuinfo": "masked"}

    def environment(self):
        env = {"PATH": "/build/toolchain/bin:/usr/local/bin:/usr/bin:/bin", "HOME": "/build/home", "TMPDIR": "/tmp",
               "CARGO_HOME": "/build/cargo", "CARGO_TARGET_DIR": "/build/out",
               "CARGO_BUILD_BUILD_DIR": "/build/out/build", "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never"}
        env.update(self.plan.allowlisted())
        return env

    def manifest(self):
        return "/build/src/" + BK.relative(self.plan.manifest, self.plan.src)

    def wrap(self, command):
        src = self.plan.src
        cpuinfo = self.work / "cpuinfo"
        if not cpuinfo.is_file():
            cpuinfo.write_text("processor\t: 0\nflags\t\t:\n")
        args = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session", "--proc", "/proc", "--dev", "/dev",
                "--tmpfs", "/tmp", "--ro-bind", str(cpuinfo), "/proc/cpuinfo"]
        for top in self.SYSTEM:
            path = self.sysroot[0] / top
            if os.path.islink(path):
                args += ["--symlink", os.readlink(path), "/" + top]
            elif path.is_dir():
                args += ["--ro-bind", str(path), "/" + top]
        args += ["--ro-bind", str(self.toolchain.directory), "/build/toolchain",
                 "--bind", str(self.work / "cargo"), "/build/cargo", "--tmpfs", "/build/src"]
        for rel in self.plan.exposed():
            args += ["--ro-bind", str(src / rel), "/build/src/" + rel]
        args += ["--bind", str(self.out), "/build/out", "--tmpfs", "/build/home",
                 "--chdir", os.path.dirname(self.manifest()), "--"]
        return args + command


class SandboxExec:
    """macOS: paths cannot be remounted, so canonical paths are real host paths."""
    kind = "sandbox-exec"
    SYSTEM = ("/usr", "/bin", "/sbin", "/System", "/Library", "/private/etc", "/private/var/db",
              "/private/var/select", "/Applications/Xcode.app", "/dev")

    def __init__(self, plan, work, out, toolchain, sysroot):
        self.plan = plan
        self.work = work
        self.out = out
        self.toolchain = toolchain
        self.paths = {"src": str(plan.src), "out": str(out), "home": str(work / "home"),
                      "cargo": str(work / "cargo"), "toolchain": str(toolchain.directory)}

    def identity(self):
        return {"kind": self.kind, "paths": self.paths, "sysroot": darwin_identity(),
                "exposed": self.plan.exposed(), "writable": ["<out>", "<home>"], "network": False}

    def environment(self):
        home = self.work / "home"
        env = {"PATH": "%s/bin:/usr/bin:/bin" % self.toolchain.directory, "HOME": str(home),
               "TMPDIR": str(self.work / "tmp"), "CARGO_HOME": str(self.work / "cargo"),
               "CARGO_TARGET_DIR": str(self.out), "CARGO_BUILD_BUILD_DIR": str(self.out / "build"),
               "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never"}
        env.update(self.plan.allowlisted())
        return env

    def manifest(self):
        return str(self.plan.manifest)

    def profile(self):
        reads = [p for p in self.SYSTEM if os.path.exists(p)]
        reads += [str(self.toolchain.directory), str(self.work / "cargo"), str(self.out),
                  str(self.work / "home"), str(self.work / "tmp")]
        reads += [str(self.plan.src / rel) for rel in self.plan.exposed()]
        ancestors = set()
        for path in reads + [os.path.dirname(self.manifest())]:
            ancestors.update(str(parent) for parent in Path(path).parents)
        lines = ["(version 1)", "(deny default)", "(allow process*)", "(allow sysctl-read)", "(allow mach-lookup)",
                 "(allow ipc-posix*)", "(allow signal)", "(allow file-read-metadata)", "(allow file-ioctl)",
                 "(allow file-read*" + "".join('\n  (literal "%s")' % p for p in sorted(ancestors))
                 + "".join('\n  (subpath "%s")' % p for p in reads) + ")",
                 '(allow file-write* (subpath "%s") (subpath "%s") (subpath "%s") (subpath "%s")'
                 ' (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))'
                 % (self.out, self.work / "home", self.work / "tmp", self.work / "cargo"),
                 "(deny network*)", ""]
        return "\n".join(lines)

    def wrap(self, command):
        return ["sandbox-exec", "-p", self.profile()] + command


def darwin_identity():
    first = BK._first_line
    return {"xcrun-sdk-version": first(["xcrun", "--show-sdk-version"]),
            "xcrun-sdk-build": first(["xcrun", "--show-sdk-build-version"]),
            "xcode": first(["xcodebuild", "-version"]), "clang": first(["clang", "--version"])}


# ---- driver ----------------------------------------------------------------

def unit_slug(unit):
    return "-".join(filter(None, [unit.package, unit.kind, unit.name if unit.kind == "bin" else None,
                                  unit.profile, unit.target]))


class Producer:
    def __init__(self, options, environ):
        self.options = options
        self.environ = environ
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
        self.rustup_home = Path(os.path.realpath(environ.get("RUSTUP_HOME") or Path.home() / ".rustup"))
        # Host-side cargo/rustc calls (fetch, metadata, -vV) must not let the
        # caller's HOME pick a different toolchain or cache.
        self.environ = dict(environ, RUSTUP_HOME=str(self.rustup_home), CARGO_HOME=str(self.host_cargo))
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
        self.toolchain = Toolchain(self.unit.manifest.parent, self.environ)

    def prepare(self):
        """Fresh checkout, staged cargo home and empty HOME for one build, always at the same paths."""
        work = self.work / "current"
        if work.exists():
            shutil.rmtree(work)
        src = work / "src"
        clean_checkout(self.repo, self.commit, src)
        # The only networked step: fill the host cargo cache for the lock.
        sh(["cargo", "fetch", "--locked", "--manifest-path", str(src / BK.relative(self.unit.manifest, self.repo))],
           env=self.environ)
        plan = Plan(self.unit, self.repo, self.commit, src, self.options.edges, self.environ, self.host_cargo)
        (work / "home").mkdir()
        (work / "tmp").mkdir()
        return work, plan

    def sandbox(self, plan, work, out):
        cls = SandboxExec if sys.platform == "darwin" else Bubblewrap
        return cls(plan, work, out, self.toolchain, self.sysroot)

    def key(self, plan, sandbox):
        document = plan.document(sandbox, self.toolchain)
        return hashlib.sha256(BK.canonical(document).encode("utf-8")).hexdigest(), document

    def build(self, plan, sandbox, out):
        if out.exists():
            raise Miss("output dir %s exists; hermetic builds start from an empty output dir" % out)
        out.mkdir(parents=True)
        started = time.time()
        (sandbox.work / "cargo").mkdir(parents=True, exist_ok=True)
        stage_cargo_home(plan, self.host_cargo, sandbox.work / "cargo")
        staged = time.time() - started
        command = self.unit.cargo_args()
        command[command.index("--manifest-path") + 1] = sandbox.manifest()
        command += ["--offline", "--locked"]
        done = subprocess.run(sandbox.wrap(command), cwd=str(plan.manifest.parent), env=sandbox.environment(),
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
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
            if BK.parse_package_id(message["package_id"])[1] != plan.unit.package:
                continue
            names = ([message["executable"]] if message.get("executable") else []) + message.get("filenames", [])
            for name in names:
                host = str(out) + name[len(sandbox.paths["out"]):] if name.startswith(sandbox.paths["out"]) else name
                rel = name[len(sandbox.paths["out"]) + 1:] if name.startswith(sandbox.paths["out"]) else name
                outputs[rel] = BK.sha256_file(host)
                if name == message.get("executable") or not rel.startswith("build/"):
                    final[rel] = outputs[rel]
        if not outputs:
            raise Miss("the build produced no artifact for %s" % plan.unit.package)
        return {"outputs": outputs, "final": final or outputs, "seconds": round(seconds, 1),
                "staging_seconds": round(staged, 1)}

    def diagnose(self, plan, sandbox, out):
        """Run the compiler's dep-info discovery inside the sandbox; flag reads outside the exposed set."""
        command = self.unit.cargo_args()
        command[command.index("--manifest-path") + 1] = sandbox.manifest()
        command += ["--offline", "--locked"]
        done = subprocess.run(sandbox.wrap(command), cwd=str(plan.manifest.parent), env=sandbox.environment(),
                              text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if done.returncode:
            raise Miss("diagnostic build failed: %s" % done.stderr[-2000:])
        paths = sandbox.paths
        to_host = lambda p: str(out) + p[len(paths["out"]):] if p.startswith(paths["out"]) else p
        artifacts = [json.loads(l) for l in done.stdout.splitlines() if l.startswith("{")]
        artifacts = [m for m in artifacts if m.get("reason") == "compiler-artifact"]
        deps_dirs = sorted({str(Path(to_host(f)).parent) for m in artifacts for f in m.get("filenames", [])
                            if Path(f).parent.name == "deps"})
        reads, outside, generated, env = set(), set(), 0, set()
        exposed = plan.exposed()
        for message in artifacts:
            message = dict(message, filenames=[to_host(f) for f in message.get("filenames", [])],
                           executable=to_host(message["executable"]) if message.get("executable") else None)
            files, names = BK.parse_dep_info(BK.dep_info_for_artifact(message, deps_dirs))
            env.update(names)
            for name in files:
                name = os.path.normpath(os.path.join(os.path.dirname(sandbox.manifest()), name))
                if name.startswith(paths["src"] + "/"):
                    rel = name[len(paths["src"]) + 1:]
                    reads.add(rel)
                    if not any(rel == e or rel.startswith(e + "/") for e in exposed):
                        outside.add(rel)
                elif name.startswith(paths["out"] + "/"):
                    generated += 1
                elif not name.startswith((paths["cargo"] + "/", paths["toolchain"] + "/")):
                    outside.add(name)
        print("diagnose: %d repository files read, %d generated, %d env names" % (len(reads), generated, len(env)))
        if outside:
            raise Miss("compiler read outside the exposed set: " + ", ".join(sorted(outside)))


def produce(options, environ):
    """Key the unit; unless --key-only, build it twice in fresh sandboxes at identical paths and compare."""
    producer = Producer(options, environ)
    base_out = Path(os.path.realpath(options.out)) if options.out else producer.repo / "target-hermetic" / unit_slug(producer.unit)
    if base_out.exists() and not options.key_only:
        raise Miss("output dir %s exists; hermetic builds start from an empty output dir" % base_out)
    current = base_out / "current"
    work, plan = producer.prepare()
    sandbox = producer.sandbox(plan, work, current)
    key, document = producer.key(plan, sandbox)
    receipt = {"key": key, "commit": producer.commit, "unit": document["unit"], "document": document,
               "sysroot_verify_seconds": round(producer.sysroot_seconds, 1),
               "toolchain_digest_seconds": round(producer.toolchain.seconds, 1),
               "cpu_detection": cpu_detection(plan.build_scripts)}
    if options.key_only:
        return receipt, producer
    builds = [producer.build(plan, sandbox, current)]
    if options.diagnose:
        producer.diagnose(plan, sandbox, current)
    current.rename(base_out / "a")
    if not options.single:
        work, plan_b = producer.prepare()
        sandbox_b = producer.sandbox(plan_b, work, current)
        key_b, _ = producer.key(plan_b, sandbox_b)
        if key_b != key:
            raise Miss("the second preparation keyed differently (%s != %s)" % (key_b[:12], key[:12]))
        builds.append(producer.build(plan_b, sandbox_b, current))
        current.rename(base_out / "b")
        # Two identical builds on one host cannot prove independence from
        # the host CPU; crates that read it at build time stay non-reusable.
        receipt["reusable"] = builds[0]["final"] == builds[1]["final"] and not receipt["cpu_detection"]
    else:
        receipt["reusable"] = False
    receipt["builds"] = builds
    receipt["out"] = str(base_out)
    return receipt, producer


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command")
    seal = sub.add_parser("seal-sysroot", help="record a sysroot rootfs: its image id and tree digest")
    seal.add_argument("directory")
    seal.add_argument("--image", required=True, help="container image id or digest the rootfs was exported from")
    make = sub.add_parser("make-sysroot", help="docker build + export + seal the pinned sysroot image")
    make.add_argument("directory")
    make.add_argument("--dockerfile", default=str(Path(__file__).with_name("hermetic-sysroot.Dockerfile")))
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
    parser.add_argument("--work", help="host dir for checkouts, staged caches and HOME (default: temp; macOS: %s)"
                        % DARWIN_WORK)
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument("--allow-dirty", action="store_true", help="key HEAD even if the worktree is dirty")
    parser.add_argument("--key-only", action="store_true", help="compute the key without building")
    parser.add_argument("--single", action="store_true", help="build once; the receipt marks the unit non-reusable")
    parser.add_argument("--diagnose", action="store_true", help="after building, list dep-info reads")
    parser.add_argument("--receipt", help="write key, document, both builds' digests and reusability as JSON")
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
    if not (options.manifest_path and options.package and options.kind):
        parser.error("--manifest-path, --package and --kind are required")
    producer = None
    try:
        receipt, producer = produce(options, dict(os.environ))
        document = receipt["document"]
        if options.json:
            Path(options.json).write_text(BK.canonical(document) + "\n")
        summary = "key=%s trees=%d files=%d lock=%d packages=%d sandbox=%s" % (
            receipt["key"], len(document["trees"]), len(document["files"]), len(document["lock"]),
            len(document["closure"]), document["sandbox"]["kind"])
        if options.key_only:
            print(summary)
        else:
            builds = receipt["builds"]
            print(summary + " reusable=%s seconds=%s%s" % (
                receipt["reusable"], "+".join("%.0f" % b["seconds"] for b in builds),
                " cpu-detection=" + ",".join(receipt["cpu_detection"]) if receipt["cpu_detection"] else ""))
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
