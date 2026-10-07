#!/usr/bin/env python3
"""Compute the content-addressed input key of one Cargo build unit.

A unit is (package, kind bin|lib|test, target triple, profile, features,
version layer). Its key is the sha256 of a canonical JSON document holding
every input the compiler recorded while building it:

  trees      the complete tree of every path package in the closure (git blob
             ids; untracked files as sha256): sources, manifests, build.rs,
             tests, assets and anything else cargo's target discovery or a
             build script could read inside the package
  files      repository files read from outside those trees (dep-info of each
             crate, build-script rerun-if-changed paths, declared edges from
             scripts/build-key-edges.json), hashed the same way
  generated  files rustc read from build-script output directories, by content
  git        files rustc read from git-dependency checkouts, by content
  env        every environment variable read at compile time (`env!`,
             rerun-if-env-changed, literal reads in build scripts and
             proc-macros) with its current value
  lock       the Cargo.lock entries of the closure only (registry checksums,
             git revisions), so unrelated lock edits keep the key
  closure    the resolved feature set of every package in the closure
  toolchain  rustc -vV, cargo -V, every .cargo/config.toml cargo reads,
             rust-toolchain.toml, the platform SDK
  build_env  RUSTFLAGS, CARGO_PROFILE_*, C toolchain variables and
             ELASTOS_RELEASE_VERSION (or "unversioned")
  edges      the blob id of the edges map itself

Inputs come from `cargo build --message-format=json` on an existing build in
the current target dir (a cold tree is compiled first). Producers write the
input list with --inputs-out; a consumer without a build computes the same key
from that list with --inputs, hashing its own trees and files: any file added
to, removed from or changed inside a listed package tree, any listed file, any
lock entry of the closure or the edges map changes the key.

Fail closed: an input outside the repository (other than the registry, git
checkouts and the toolchain), an unreadable file, a cross-package include not
declared in the edges map, a build script or proc-macro whose environment or
filesystem reads cannot be enumerated, or a lock entry without a checksum
yields no key (exit 3) and a reason, never a partial key.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

VERSION = 2
KINDS = ("bin", "lib", "test")
EXIT_MISS = 3
# Set by cargo for rustc and build scripts; their values derive from inputs
# already in the key (manifests, profile, target) or name a location.
CARGO_SET = {"CARGO", "OUT_DIR", "TARGET", "HOST", "PROFILE", "OPT_LEVEL", "DEBUG", "NUM_JOBS",
             "RUSTC", "RUSTDOC", "RUSTC_LINKER", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER"}
BUILD_ENV = ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_BUILD_TARGET",
             "CARGO_INCREMENTAL", "CC", "CXX", "AR", "CFLAGS", "CXXFLAGS", "LDFLAGS", "RUSTC_WRAPPER")
BUILD_ENV_PREFIXES = ("CARGO_PROFILE_", "CARGO_TARGET_", "CC_", "CXX_", "AR_", "CFLAGS_", "CXXFLAGS_",
                      "TARGET_CC", "TARGET_CXX", "TARGET_AR", "TARGET_CFLAGS", "TARGET_CXXFLAGS")
# Location-only settings: where artifacts land never changes their bytes.
BUILD_ENV_IGNORED = ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR")

# Environment and filesystem access in build scripts and proc-macros, whose
# reads happen outside rustc's dep-info. Literal names become inputs; anything
# else makes the crate opaque.
ENV_LITERAL = re.compile(r'\b(?:var|var_os)\s*\(\s*"([^"\\]+)"|\b(?:option_)?env!\s*\(\s*"([^"\\]+)"')
ENV_DYNAMIC = re.compile(r'\b(?:var|var_os)\s*\(\s*[^"\s)]|\benv::vars(?:_os)?\s*\(|\bvars(?:_os)?\s*\(\s*\)')
FS_ACCESS = re.compile(r'\b(?:fs::|File::open|File::create|read_to_string\s*\(|read_dir\s*\(|include_dir!|'
                       r'OpenOptions::new|Command::new)')


class Miss(Exception):
    """The key cannot be computed; the message says why. The unit must be rebuilt."""


class Unit:
    def __init__(self, manifest, package, kind, name=None, target=None, profile=None,
                 features=(), no_default_features=False):
        if kind not in KINDS:
            raise Miss("unknown unit kind %r" % kind)
        self.manifest = Path(manifest).resolve()
        self.package = package
        self.kind = kind
        self.name = name or (package if kind == "bin" else None)
        self.target = target
        self.profile = profile or ("test" if kind == "test" else "dev")
        self.features = sorted(set(features))
        self.no_default_features = bool(no_default_features)

    def describe(self, root):
        return {"package": self.package, "kind": self.kind, "name": self.name, "target": self.target,
                "profile": self.profile, "features": self.features,
                "no_default_features": self.no_default_features,
                "workspace": relative(self.manifest.parent, root)}

    def cargo_args(self):
        args = ["cargo", "test", "--no-run"] if self.kind == "test" else ["cargo", "build"]
        args += ["--message-format=json-render-diagnostics", "--manifest-path", str(self.manifest),
                 "-p", self.package, "--profile", self.profile]
        if self.kind == "bin":
            args += ["--bin", self.name]
        elif self.kind == "lib":
            args += ["--lib"]
        if self.target:
            args += ["--target", self.target]
        if self.features:
            args += ["--features", ",".join(self.features)]
        if self.no_default_features:
            args += ["--no-default-features"]
        return args


def relative(path, root):
    try:
        return Path(os.path.realpath(path)).relative_to(root).as_posix()
    except ValueError:
        return None


def run(args, cwd, env=None):
    done = subprocess.run(args, cwd=str(cwd), env=env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if done.returncode:
        raise Miss("%s failed (%d): %s" % (" ".join(args[:2]), done.returncode, done.stderr.strip()[-2000:]))
    return done.stdout


def read_bytes(path):
    try:
        return Path(path).read_bytes()
    except OSError as error:
        raise Miss("unreadable input %s: %s" % (path, error.strerror))


def git_blob(path):
    data = read_bytes(path)
    return hashlib.sha1(b"blob %d\0" % len(data) + data).hexdigest()


def sha256_file(path):
    return "sha256:" + hashlib.sha256(read_bytes(path)).hexdigest()


def repo_root(start):
    return Path(os.path.realpath(run(["git", "rev-parse", "--show-toplevel"], start).strip()))


def tracked_files(root):
    out = subprocess.run(["git", "ls-files", "-z"], cwd=str(root), text=True, stdout=subprocess.PIPE, check=True)
    return set(out.stdout.split("\0")) - {""}


def listed_tree(root, directory):
    """Files cargo would treat as the package: tracked plus untracked, not ignored."""
    out = run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", directory], root)
    return sorted(set(out.split("\0")) - {""})


def unescape_make(token):
    return re.sub(r"\\([ #\\])", r"\1", token)


def parse_dep_info(path):
    """Return (files, env names) rustc recorded in a Makefile-style .d file."""
    try:
        text = Path(path).read_text(errors="surrogateescape")
    except OSError as error:
        raise Miss("unreadable dep-info %s: %s" % (path, error.strerror))
    files = []
    env = []
    for line in text.splitlines():
        if line.startswith("# env-dep:"):
            env.append(line[len("# env-dep:"):].split("=", 1)[0])
        elif not files and not line.startswith("#"):
            # "<output>: <dep> <dep>..." ; later "<dep>:" rules repeat the same set.
            head, _, rest = line.partition(": ")
            if rest.strip():
                files.extend(unescape_make(token) for token in re.split(r"(?<!\\) +", rest.strip()) if token)
    return files, env


def canonical(document):
    """Unambiguous JSON: sorted keys, no whitespace, ASCII-escaped so lone surrogates from non-UTF-8
    environment values or names are encoded exactly instead of failing or colliding."""
    return json.dumps(document, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False)


# ---- input discovery -------------------------------------------------------

def parse_package_id(package_id):
    """'registry+URL#name@ver' | 'path+file:///dir#ver' | 'git+URL?rev=..#name@ver' -> (source, name, version)."""
    source, _, fragment = package_id.partition("#")
    if "@" in fragment:
        name, _, version = fragment.rpartition("@")
    else:
        name, version = source.rstrip("/").rsplit("/", 1)[-1].split("?")[0], fragment
    return source, name, version


def package_dir_of(package_id):
    source, _, _ = parse_package_id(package_id)
    if source.startswith("path+file://"):
        return os.path.realpath(source[len("path+file://"):])
    return None


def dep_info_for_artifact(message, deps_dirs):
    """Locate the rustc .d file of one compiler-artifact message."""
    paths = [Path(f) for f in message.get("filenames", [])]
    if message.get("executable"):
        paths.insert(0, Path(message["executable"]))
    for path in paths:
        if path.parent.name == "deps":
            stem = path.name
            for suffix in (".rlib", ".rmeta", ".so", ".dylib", ".dll", ".wasm", ".a", ".exe"):
                if stem.endswith(suffix):
                    stem = stem[:-len(suffix)]
                    break
            if stem.startswith("lib") and path.suffix in (".rlib", ".rmeta", ".so", ".dylib", ".a"):
                stem = stem[3:]
            candidate = path.parent / (stem + ".d")
            if candidate.is_file():
                return candidate
        if path.name.startswith("build-script-") and path.parent.parent.name == "build":
            found = sorted(path.parent.glob("*.d"))
            if len(found) == 1:
                return found[0]
    # Uplifted root artifacts (bins, cdylibs) carry no metadata hash: match
    # the bytes against the hashed originals in deps/ (cargo copies on macOS).
    crate = message["target"]["name"].replace("-", "_")
    candidates = set()
    for path in paths:
        for deps_dir in deps_dirs:
            for original in Path(deps_dir).glob(crate + "-*" + path.suffix):
                if original.suffix == ".d" or not original.is_file() or original.stem.count(".") > 0:
                    continue
                if original.stat().st_ino == path.stat().st_ino or _same_bytes(original, path):
                    candidates.add(original.with_suffix(".d"))
    candidates = sorted(c for c in candidates if c.is_file())
    if len(candidates) == 1:
        return candidates[0]
    if candidates:
        raise Miss("ambiguous dep-info for %s: %s" % (message["package_id"], ", ".join(map(str, candidates))))
    raise Miss("no dep-info for artifact %s (%s)" % (message["package_id"], ", ".join(map(str, paths))))


def _same_bytes(a, b):
    try:
        if a.stat().st_size != b.stat().st_size:
            return False
        return a.read_bytes() == b.read_bytes()
    except OSError:
        return False


def build_script_declarations(out_dir):
    """rerun-if-changed paths and rerun-if-env-changed names from a build script's captured stdout."""
    output = Path(out_dir).parent / "output"
    try:
        lines = output.read_text(errors="replace").splitlines()
    except OSError as error:
        raise Miss("unreadable build-script output %s: %s" % (output, error.strerror))
    paths, env = [], []
    for line in lines:
        for prefix in ("cargo::", "cargo:"):
            if line.startswith(prefix):
                directive, _, value = line[len(prefix):].partition("=")
                if directive == "rerun-if-changed":
                    paths.append(value)
                elif directive == "rerun-if-env-changed":
                    env.append(value)
                break
    return paths, env


def scan_sources(sources, what, name, filesystem, dynamic_ok):
    """Environment names a build script or proc-macro reads by literal; Miss on dynamic access."""
    names = set()
    for source in sources:
        if not str(source).endswith(".rs"):
            continue
        try:
            text = Path(source).read_text(errors="replace")
        except OSError as error:
            raise Miss("unreadable %s source %s: %s" % (what, source, error.strerror))
        text = re.sub(r"//[^\n]*", "", text)
        for literal in ENV_LITERAL.finditer(text):
            names.add(literal.group(1) or literal.group(2))
        if not dynamic_ok and ENV_DYNAMIC.search(text):
            raise Miss("%s %s reads the environment dynamically (%s); its inputs cannot be enumerated"
                       % (what, name, source))
        if filesystem and FS_ACCESS.search(text):
            raise Miss("%s %s reads the filesystem or runs commands at expansion (%s); rustc does not "
                       "track those reads" % (what, name, source))
    return names


def discover_inputs(unit, root, environ):
    """Run cargo on the unit (fresh when already built) and collect the compiler's record of inputs."""
    workspace = unit.manifest.parent
    out = run(unit.cargo_args(), workspace, env=environ)
    artifacts, executed = [], []
    for line in out.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact":
            artifacts.append(message)
        elif message.get("reason") == "build-script-executed":
            executed.append(message)
    if not artifacts:
        raise Miss("cargo produced no artifacts for the unit")
    built = sum(1 for m in artifacts if not m.get("fresh"))
    deps_dirs = sorted({str(Path(f).parent) for m in artifacts for f in m.get("filenames", [])
                        if Path(f).parent.name == "deps"})
    if not deps_dirs:
        raise Miss("no deps/ directory among the unit's artifacts")
    # Every "<...>/<profile>" directory holding compiler outputs.
    generated_roots = {str(Path(d).parent) for d in deps_dirs}
    generated_roots.update(str(Path(m["out_dir"]).parent.parent.parent) for m in executed)
    generated_roots.update(str(Path(f).parent) for m in artifacts for f in m.get("filenames", [])
                           if Path(f).parent.name != "deps" and m["target"]["kind"] != ["custom-build"])

    raw_files = []        # (absolute path, package id)
    env_names = {}        # name -> "cargo" | "env"
    closure = {}          # package id -> sorted features
    for message in executed:
        package_id = message["package_id"]
        paths, env = build_script_declarations(message["out_dir"])
        for name in env:
            env_names[name] = "env"
        # Registry and git packages are immutable per lock entry; only path
        # packages can change under a rerun-if-changed path (their whole
        # tree is keyed, so only paths outside it matter here).
        package_dir = package_dir_of(package_id)
        if package_dir is not None:
            for path in paths:
                raw_files.append((os.path.normpath(os.path.join(package_dir, path)), package_id))
    for message in artifacts:
        package_id = message["package_id"]
        name = parse_package_id(package_id)[1]
        closure.setdefault(package_id, sorted(message.get("features", [])))
        dep_info = dep_info_for_artifact(message, deps_dirs)
        files, env = parse_dep_info(dep_info)
        sources = [os.path.normpath(os.path.join(workspace, f)) for f in files]
        for source in sources:
            raw_files.append((source, package_id))
        for variable in env:
            env_names[variable] = "cargo" if cargo_set(variable) else "env"
        kind = message["target"]["kind"]
        is_path = package_dir_of(package_id) is not None
        if kind == ["custom-build"]:
            # A build script's own env reads are invisible to cargo unless
            # declared: enumerate literal ones. A path script that reads the
            # environment dynamically is opaque; a registry script is fixed
            # by its checksum and trusted, as cargo trusts it, to declare or
            # read only cargo-provided variables.
            scanned = scan_sources(sources, "build script of", name, filesystem=False, dynamic_ok=not is_path)
        elif "proc-macro" in kind and is_path:
            # Registry proc-macros are fixed by their checksum; a path
            # proc-macro that reads files or the environment at expansion
            # time has inputs rustc never records.
            scanned = scan_sources(sources, "proc-macro", name, filesystem=True, dynamic_ok=False)
        else:
            scanned = ()
        for variable in scanned:
            env_names.setdefault(variable, "cargo" if cargo_set(variable) else "env")
    return {"raw_files": raw_files, "env": env_names, "closure": closure,
            "generated_roots": sorted(generated_roots), "built": built}


def cargo_set(name):
    return name in CARGO_SET or name.startswith("CARGO_")


# ---- classification --------------------------------------------------------

def cargo_home(environ):
    return Path(os.path.realpath(environ.get("CARGO_HOME") or Path.home() / ".cargo"))


def sysroot(unit, environ):
    return Path(os.path.realpath(run(["rustc", "--print", "sysroot"], unit.manifest.parent, env=environ).strip()))


def classify(path, root, home, toolchain, generated_roots):
    """Return (class, normalized) for an absolute input path."""
    real = os.path.realpath(path)
    for generated in generated_roots:
        if real.startswith(generated + os.sep):
            below = Path(real[len(generated) + 1:]).parts
            if len(below) >= 3 and below[0] == "build":
                # build/<package>-<metadata hash>/out/... ; the hash names a
                # location, the package and the path below it name the file.
                return "generated", "generated:" + below[1].rsplit("-", 1)[0] + "/" + "/".join(below[2:])
            return "generated", None
    rel = relative(real, root)
    if rel is not None:
        return "repo", rel
    registry = str(home / "registry" / "src")
    if real.startswith(registry + os.sep):
        parts = Path(real[len(registry) + 1:]).parts
        if len(parts) >= 2:
            return "registry", "registry:" + "/".join(parts[1:])
    checkouts = str(home / "git" / "checkouts")
    if real.startswith(checkouts + os.sep):
        return "git", "git:" + Path(real[len(checkouts) + 1:]).as_posix()
    if real == str(toolchain) or real.startswith(str(toolchain) + os.sep):
        return "toolchain", "toolchain:" + Path(real[len(str(toolchain)) + 1:]).as_posix()
    return "foreign", real


def load_edges(path):
    if not Path(path).is_file():
        return {"packages": {}, "units": {}}, None
    try:
        edges = json.loads(Path(path).read_text())
    except (OSError, ValueError) as error:
        raise Miss("edges map %s unreadable: %s" % (path, error))
    edges.setdefault("packages", {})
    edges.setdefault("units", {})
    return edges, git_blob(path)


def declared(entries, rel):
    for entry in entries:
        entry = entry.rstrip("/")
        if rel == entry or rel.startswith(entry + "/"):
            return True
    return False


def collect_inputs(unit, root, discovered, edges, environ):
    """Turn raw compiler records into the repo-relative input list (the manifest)."""
    home = cargo_home(environ)
    toolchain = sysroot(unit, environ)
    files = set()
    trees = set()
    generated = {}
    git_files = {}
    registry_names = set()
    package_dirs = {}
    closure = {}
    for package_id, features in discovered["closure"].items():
        directory = package_dir_of(package_id)
        if directory:
            rel = relative(directory, root)
            if rel is None:
                raise Miss("path package %s lies outside the repository" % package_id)
            package_dirs[package_id] = rel
            trees.add(rel)
            closure["path:%s#%s" % (rel, parse_package_id(package_id)[2])] = features
        else:
            closure[package_id] = features

    def outside_trees(rel):
        return not any(rel == tree or rel.startswith(tree + "/") for tree in trees)

    if outside_trees(relative(unit.manifest, root)):
        files.add(relative(unit.manifest, root))
    undeclared = []
    for path, package_id in discovered["raw_files"]:
        kind, normalized = classify(path, root, home, toolchain, discovered["generated_roots"])
        source, name, version = parse_package_id(package_id)
        if kind == "toolchain":
            continue
        if kind == "generated":
            if normalized is None:
                raise Miss("generated input %s read by %s is not a build-script output" % (path, package_id))
            digest = sha256_file(path)
            if generated.get(normalized, digest) != digest:
                raise Miss("generated input %s has two different contents in this build" % normalized)
            generated[normalized] = digest
        elif kind == "repo":
            owner = package_dirs.get(package_id)
            if owner is not None and normalized != owner and not normalized.startswith(owner + "/"):
                if not declared(edges["packages"].get(name, []), normalized):
                    undeclared.append("%s (read by %s)" % (normalized, name))
            if outside_trees(normalized):
                files.add(normalized)
        elif kind == "registry":
            if not source.startswith("registry+"):
                raise Miss("registry file %s read by non-registry package %s" % (path, package_id))
            registry_names.add(normalized[len("registry:"):].split("/", 1)[0])
        elif kind == "git":
            if not source.startswith("git+"):
                raise Miss("git checkout file %s read by non-git package %s" % (path, package_id))
            git_files[normalized] = sha256_file(path)
        else:
            raise Miss("undeclared absolute input %s read by %s" % (path, package_id))
    if undeclared:
        raise Miss("undeclared cross-package inputs; declare them under packages.<name> in the edges map: "
                   + ", ".join(sorted(undeclared)))
    unit_edges = edges["units"].get(unit.package + "/" + unit.kind, {})
    env = dict(discovered["env"])
    for path in unit_edges.get("paths", []):
        files.add(path)
    for name in unit_edges.get("env", []):
        env[name] = "env"
    for name, version in sorted((parse_package_id(p)[1], parse_package_id(p)[2]) for p in discovered["closure"]):
        registry_names.discard("%s-%s" % (name, version))
    if registry_names:
        raise Miss("registry sources outside the closure: %s" % ", ".join(sorted(registry_names)))
    return {"version": VERSION, "unit": unit.describe(root), "files": sorted(files), "trees": sorted(trees),
            "generated": generated, "git": git_files, "env": env, "closure": closure}


# ---- lock slice ------------------------------------------------------------

def parse_lock(path):
    """Minimal Cargo.lock reader: [[package]] blocks with name/version/source/checksum."""
    try:
        text = Path(path).read_text()
    except OSError as error:
        raise Miss("unreadable lock file %s: %s" % (path, error.strerror))
    packages = []
    current = None
    for line in text.splitlines():
        if line.strip() == "[[package]]":
            current = {}
            packages.append(current)
        elif current is not None:
            match = re.match(r'^(name|version|source|checksum) = "(.*)"$', line)
            if match:
                current[match.group(1)] = match.group(2)
    return packages


def lock_slice_of(lock, closure):
    """Checksums (registry) and resolved revisions (git) of the closure's non-path packages."""
    by_name = {}
    for entry in lock:
        by_name.setdefault((entry.get("name"), entry.get("version")), []).append(entry)
    result = {}
    for package_id in closure:
        if package_id.startswith("path:"):
            continue
        source, name, version = parse_package_id(package_id)
        matches = [e for e in by_name.get((name, version), []) if e.get("source", "").startswith(source.split("?")[0])]
        if len(matches) != 1:
            raise Miss("lock entry for %s not found or ambiguous (%d)" % (package_id, len(matches)))
        entry = matches[0]
        if source.startswith("registry+"):
            if not entry.get("checksum"):
                raise Miss("lock entry for %s has no checksum" % package_id)
            result[package_id] = entry["checksum"]
        elif source.startswith("git+"):
            if not re.search(r"#[0-9a-f]{40}$", entry.get("source", "")):
                raise Miss("lock entry for %s has no resolved git revision" % package_id)
            result[package_id] = entry["source"]
        else:
            raise Miss("unknown package source %s" % package_id)
    return result


# ---- environment -----------------------------------------------------------

def toolchain_identity(unit, root, environ):
    workspace = unit.manifest.parent
    identity = {"rustc": run(["rustc", "-vV"], workspace, env=environ).strip(),
                "cargo": run(["cargo", "-V"], workspace, env=environ).strip(),
                "configs": {}}
    for directory in [workspace] + list(workspace.parents):
        for name in ("config.toml", "config"):
            candidate = directory / ".cargo" / name
            if candidate.is_file():
                rel = relative(candidate, root)
                if rel is None:
                    identity["configs"][str(candidate)] = sha256_file(candidate)
                else:
                    identity["configs"][rel] = git_blob(candidate)
        if directory == root:
            break
    for name in ("config.toml", "config"):
        candidate = cargo_home(environ) / name
        if candidate.is_file():
            identity["configs"]["$CARGO_HOME/" + name] = sha256_file(candidate)
    pin = root / "rust-toolchain.toml"
    identity["rust-toolchain.toml"] = git_blob(pin) if pin.is_file() else None
    identity["sdk"] = sdk_identity()
    return identity


def _first_line(args):
    try:
        done = subprocess.run(args, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    except OSError:
        return None
    return done.stdout.splitlines()[0].strip() if done.stdout else None


def sdk_identity():
    if sys.platform == "darwin":
        return {"xcrun-sdk-version": _first_line(["xcrun", "--show-sdk-version"]),
                "xcrun-sdk-build": _first_line(["xcrun", "--show-sdk-build-version"])}
    # openssl-sys links the system library it finds through pkg-config; the
    # installed package list identifies the rest of the sysroot (runner image).
    return {"cc": _first_line(["cc", "--version"]), "libc": _first_line(["ldd", "--version"]),
            "openssl": _first_line(["pkg-config", "--modversion", "openssl"]),
            "packages": _digest(["dpkg-query", "-W", "-f", "${Package}=${Version}\\n"])}


def _digest(args):
    try:
        done = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    except OSError:
        return None
    return hashlib.sha256(done.stdout).hexdigest() if done.returncode == 0 else None


def build_environment(environ):
    values = {}
    for name, value in environ.items():
        if name in BUILD_ENV_IGNORED:
            continue
        if name in BUILD_ENV or name.startswith(BUILD_ENV_PREFIXES):
            values[name] = value
    values["ELASTOS_RELEASE_VERSION"] = environ.get("ELASTOS_RELEASE_VERSION") or "unversioned"
    return values


def normalize_value(value, root):
    if value is None:
        return None
    if os.path.isabs(value):
        rel = relative(value, root)
        if rel is not None:
            return "repo:" + rel
    return value


# ---- key -------------------------------------------------------------------

def hash_entry(root, rel, tracked):
    path = root / rel
    if not path.is_file():
        raise Miss("input %s is missing" % rel)
    return git_blob(path) if rel in tracked else sha256_file(path)


def resolve(inputs, root, environ, unit, edges_blob):
    """Hash the manifest's inputs against the current tree and environment."""
    tracked = tracked_files(root)
    files = {}
    for rel in inputs["files"]:
        if (root / rel).is_dir():
            for entry in listed_tree(root, rel):
                files[entry] = hash_entry(root, entry, tracked)
        else:
            files[rel] = hash_entry(root, rel, tracked)
    trees = {}
    for rel in inputs["trees"]:
        if not (root / rel).is_dir():
            raise Miss("package directory %s is missing" % rel)
        trees[rel] = {entry: hash_entry(root, entry, tracked) for entry in listed_tree(root, rel)
                      if "/target/" not in "/" + entry + "/"}
    env = {}
    for name, origin in sorted(inputs["env"].items()):
        env[name] = "<cargo>" if origin == "cargo" else normalize_value(environ.get(name), root)
    lock = lock_slice_of(parse_lock(unit.manifest.parent / "Cargo.lock"), inputs["closure"])
    return {"version": VERSION, "unit": inputs["unit"], "files": files, "trees": trees,
            "generated": inputs["generated"], "git": inputs["git"], "env": env,
            "closure": inputs["closure"], "lock": lock, "edges": edges_blob,
            "toolchain": toolchain_identity(unit, root, environ), "build_env": build_environment(environ)}


def compute_key(unit, root, environ, edges_path=None, inputs=None):
    """Return (key, document, inputs). Raises Miss when the key cannot be computed."""
    edges, edges_blob = load_edges(edges_path if edges_path is not None else root / "scripts" / "build-key-edges.json")
    if inputs is None:
        discovered = discover_inputs(unit, root, environ)
        inputs = collect_inputs(unit, root, discovered, edges, environ)
        inputs["built"] = discovered["built"]
    elif inputs.get("version") != VERSION or inputs.get("unit") != unit.describe(root):
        raise Miss("input list does not describe this unit")
    document = resolve(inputs, root, environ, unit, edges_blob)
    return hashlib.sha256(canonical(document).encode("utf-8")).hexdigest(), document, inputs


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manifest-path", required=True, help="workspace root Cargo.toml of the unit")
    parser.add_argument("--package", required=True)
    parser.add_argument("--kind", choices=KINDS, required=True)
    parser.add_argument("--name", help="binary name (bin units; default: package name)")
    parser.add_argument("--target", help="target triple (default: host)")
    parser.add_argument("--profile", help="cargo profile (default: dev, or test for test units)")
    parser.add_argument("--features", default="", help="comma-separated features")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--edges", help="edges map (default: <repo>/scripts/build-key-edges.json)")
    parser.add_argument("--inputs", help="compute from a recorded input list instead of a build")
    parser.add_argument("--inputs-out", help="write the discovered input list (for consumers without a build)")
    parser.add_argument("--json", help="write the canonical key document")
    options = parser.parse_args(argv)
    try:
        unit = Unit(options.manifest_path, options.package, options.kind, options.name, options.target,
                    options.profile, [f for f in options.features.split(",") if f], options.no_default_features)
        root = repo_root(unit.manifest.parent)
        inputs = None
        if options.inputs:
            try:
                inputs = json.loads(Path(options.inputs).read_text())
            except (OSError, ValueError) as error:
                raise Miss("input list %s unreadable: %s" % (options.inputs, error))
        key, document, inputs = compute_key(unit, root, dict(os.environ), options.edges, inputs)
    except Miss as miss:
        print("key=None reason=%s" % miss)
        return EXIT_MISS
    if options.inputs_out:
        Path(options.inputs_out).write_text(json.dumps(inputs, indent=1, sort_keys=True) + "\n")
    if options.json:
        Path(options.json).write_text(canonical(document) + "\n")
    print("key=%s files=%d env=%d lock=%d packages=%d generated=%d built=%s" % (
        key, len(document["files"]) + sum(len(t) for t in document["trees"].values()), len(document["env"]),
        len(document["lock"]), len(document["closure"]), len(document["generated"]), inputs.get("built", "n/a")))
    return 0


if __name__ == "__main__":
    sys.exit(main())
