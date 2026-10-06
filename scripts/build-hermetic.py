#!/usr/bin/env python3
"""Build one Cargo unit hermetically and key it by construction.

The unit (package, kind bin|lib|test, target triple, profile, features,
version layer) is built from a clean checkout of HEAD (git archive: no
untracked or ignored file exists), with the environment cleared to an
explicit allowlist, inside a sandbox that exposes read-only exactly:

  - the path packages of the unit's workspace resolve and their declared
    edges from scripts/build-key-edges.json, the workspace manifest, lock
    and .cargo configs, rust-toolchain.toml
  - the rustup toolchain, the cargo registry and git caches (read-only; the
    lock pins their contents), the system sysroot (/usr, /etc, ...)
  - a writable target dir, HOME and /tmp; no network

Anything else does not exist for the build: an undeclared cross-package
include, a proc-macro or build script reading an undeclared file, an
environment variable outside the allowlist, or a gitignored generated file
either fails the build (a miss) or cannot influence it.

Key = sha256 of canonical JSON: git tree ids of every exposed path package,
blob ids of the exposed root files and edges, the lock slice of the unit's
closure with resolved features, the allowlisted environment (names and
values), rustc/cargo/SDK identity, the edges map blob and the sandbox
profile hash. It needs no build to compute (--key-only), so consumers
derive the same key from the same commit, environment and toolchain.
Generated files are outputs, never inputs. Dep-info discovery
(scripts/build-key.py) remains available as a diagnostic (--diagnose) that
lists what the compiler read and flags anything outside the exposed set.

Linux uses bubblewrap (bwrap); macOS uses sandbox-exec. Exit 3 = miss.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

SPEC = importlib.util.spec_from_file_location("buildkey", Path(__file__).with_name("build-key.py"))
BK = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BK)
Miss = BK.Miss

VERSION = 1
# Caller variables that may enter the build, and therefore the key.
PASS_THROUGH = set(BK.BUILD_ENV) | {"ELASTOS_RELEASE_VERSION", "SOURCE_DATE_EPOCH", "MACOSX_DEPLOYMENT_TARGET",
                                    "SDKROOT", "DEVELOPER_DIR", "RUSTUP_TOOLCHAIN"}
PASS_THROUGH_PREFIXES = BK.BUILD_ENV_PREFIXES
ROOT_FILES = ("Cargo.toml", "Cargo.lock", ".cargo/config.toml", ".cargo/config")
REPO_FILES = (".cargo/config.toml", ".cargo/config", "rust-toolchain.toml", "rust-toolchain")


def sh(args, cwd=None, env=None, check=True):
    done = subprocess.run(args, cwd=cwd and str(cwd), env=env, text=True, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE)
    if check and done.returncode:
        raise Miss("%s failed (%d): %s" % (" ".join(map(str, args[:3])), done.returncode,
                                           done.stderr.strip()[-3000:]))
    return done


def proxies():
    """Directory of the cargo/rustc proxies on PATH (rustup's ~/.cargo/bin)."""
    cargo = shutil.which("cargo")
    if cargo is None:
        raise Miss("cargo not found on PATH")
    return Path(os.path.realpath(cargo)).parent


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
    return closure, packages, members


def normalized_id(package, root):
    if package["source"] is None:
        rel = BK.relative(Path(package["manifest_path"]).parent, root)
        return "path:%s#%s" % (rel, package["version"])
    return "%s#%s@%s" % (package["source"], package["name"], package["version"])


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


class Plan:
    """Everything the sandbox exposes and the key covers."""

    def __init__(self, unit, repo, commit, src, edges_rel, environ):
        self.unit = unit
        self.repo = repo
        self.commit = commit
        self.src = src
        self.environ = environ
        self.manifest = src / BK.relative(unit.manifest, repo)
        self.workspace = BK.relative(unit.manifest.parent, repo)
        meta = metadata(unit, self.manifest, environ)
        self.closure, packages, members = closure_of(meta, unit)
        self.closure_ids = {normalized_id(packages[i], src): f for i, f in self.closure.items()}
        edges_path = src / edges_rel
        self.edges, self.edges_blob = BK.load_edges(edges_path)
        self.edges_rel = edges_rel if edges_path.is_file() else None
        # Exposed path packages: every path package of the workspace resolve
        # (cargo loads all member manifests and path dependencies), each keyed
        # by its tree id, so nothing readable is unkeyed.
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
        for package in packages.values():
            if package["source"] is None:
                directory = Path(package["manifest_path"]).parent
                rel = BK.relative(directory, src)
                if rel is None:
                    raise Miss("path package %s lies outside the repository" % package["id"])
                self.trees.add(rel)
                # A path dependency from another workspace inherits fields from
                # that workspace's root manifest; cargo must be able to read it.
                if not self.manifest.parent == directory and not str(directory).startswith(str(self.manifest.parent) + "/"):
                    self.files.add(workspace_root_of(package["manifest_path"], roots, src, environ))
        self.edge_paths = set()
        for package_id in self.closure:
            name = packages[package_id]["name"]
            self.edge_paths.update(self.edges["packages"].get(name, []))
        unit_edges = self.edges["units"].get(unit.package + "/" + unit.kind, {})
        self.edge_paths.update(unit_edges.get("paths", []))
        self.edge_env = set(unit_edges.get("env", []))
        for rel in sorted(self.edge_paths):
            if not (src / rel).exists():
                raise Miss("declared edge %s is not in commit %s" % (rel, commit[:10]))
        self.lock = BK.lock_slice_of(BK.parse_lock(self.manifest.parent / "Cargo.lock"), self.closure_ids)

    def exposed(self):
        """Repository-relative paths bound read-only into the sandbox."""
        return sorted(self.trees | self.files | self.edge_paths)

    def environment(self, out, home, cargo_home, rustup_home):
        env = {"PATH": "%s:%s/bin:/usr/local/bin:/usr/bin:/bin" % (proxies(), cargo_home), "HOME": str(home),
               "TMPDIR": str(home / "tmp"), "CARGO_HOME": str(cargo_home), "RUSTUP_HOME": str(rustup_home),
               "CARGO_TARGET_DIR": str(out), "CARGO_BUILD_BUILD_DIR": str(out / "build"),
               "CARGO_NET_OFFLINE": "true", "CARGO_TERM_COLOR": "never"}
        for name, value in self.environ.items():
            if name in PASS_THROUGH or name.startswith(PASS_THROUGH_PREFIXES) or name in self.edge_env:
                env[name] = value
        return env

    def keyed_environment(self):
        values = {}
        for name, value in self.environ.items():
            if name in PASS_THROUGH or name.startswith(PASS_THROUGH_PREFIXES) or name in self.edge_env:
                values[name] = value
        values["ELASTOS_RELEASE_VERSION"] = self.environ.get("ELASTOS_RELEASE_VERSION") or "unversioned"
        for name in sorted(self.edge_env):
            values.setdefault(name, None)
        return values

    def document(self, sandbox):
        trees = {rel: git_id(self.repo, self.commit, rel) for rel in sorted(self.trees)}
        files = {rel: git_id(self.repo, self.commit, rel) for rel in sorted(self.files | self.edge_paths)}
        toolchain = BK.toolchain_identity(BK.Unit(self.manifest, self.unit.package, self.unit.kind,
                                                  self.unit.name, self.unit.target, self.unit.profile,
                                                  self.unit.features, self.unit.no_default_features),
                                          self.src, self.environ)
        return {"version": VERSION, "unit": self.unit.describe(self.repo), "trees": trees, "files": files,
                "closure": self.closure_ids, "lock": self.lock, "env": self.keyed_environment(),
                "toolchain": toolchain, "edges": self.edges_blob, "sandbox": sandbox}


# ---- sandboxes -------------------------------------------------------------

class Sandbox:
    """A sandbox is identified by what it exposes, not by where things live on this machine."""
    SYSTEM = ()

    def __init__(self, plan, out, scratch, cargo_home, rustup_home):
        self.plan = plan
        self.out = out
        self.home = scratch / "home"
        self.cargo_home = cargo_home
        self.rustup_home = rustup_home

    def identity(self):
        return {"kind": self.kind, "system": sorted(p for p in self.SYSTEM if os.path.exists(p)),
                "toolchain": ["<rustup-home>", "<cargo-home>/bin", "<cargo-home>/registry", "<cargo-home>/git",
                              "<proxies>"],
                "exposed": self.plan.exposed(), "writable": ["<out>", "<home>", "/tmp"], "network": False}


class Bubblewrap(Sandbox):
    kind = "bwrap"
    SYSTEM = ("/usr", "/etc", "/lib", "/lib64", "/bin", "/sbin", "/opt")

    def wrap(self, command):
        src = self.plan.src
        args = ["bwrap", "--unshare-all", "--die-with-parent", "--new-session", "--proc", "/proc", "--dev", "/dev",
                "--tmpfs", "/tmp"]
        for top in self.SYSTEM:
            if os.path.islink(top):
                args += ["--symlink", os.readlink(top), top]
            elif os.path.isdir(top):
                args += ["--ro-bind", top, top]
        args += ["--ro-bind", str(self.rustup_home), str(self.rustup_home), "--tmpfs", str(self.cargo_home)]
        if not str(proxies()).startswith((str(self.cargo_home) + "/", "/usr/", "/bin/")):
            args += ["--ro-bind", str(proxies()), str(proxies())]
        for sub in ("bin", "registry", "git", "config.toml", "config"):
            if (self.cargo_home / sub).exists():
                args += ["--ro-bind", str(self.cargo_home / sub), str(self.cargo_home / sub)]
        args += ["--tmpfs", str(src)]
        for rel in self.plan.exposed():
            args += ["--ro-bind", str(src / rel), str(src / rel)]
        args += ["--bind", str(self.out), str(self.out), "--bind", str(self.home), str(self.home),
                 "--chdir", str(self.plan.manifest.parent), "--"]
        return args + command


class SandboxExec(Sandbox):
    kind = "sandbox-exec"
    SYSTEM = ("/usr", "/bin", "/sbin", "/System", "/Library", "/private/etc", "/private/var/db",
              "/private/var/select", "/Applications/Xcode.app", "/dev")

    def profile(self):
        src = self.plan.src
        reads = [p for p in self.SYSTEM if os.path.exists(p)]
        reads += [str(self.rustup_home), str(self.cargo_home), str(proxies()), str(self.out), str(self.home)]
        reads += [str(src / rel) for rel in self.plan.exposed()]
        ancestors = set()
        for path in reads + [str(self.plan.manifest.parent)]:
            ancestors.update(str(parent) for parent in Path(path).parents)
        lines = ["(version 1)", "(deny default)", "(allow process*)", "(allow sysctl-read)", "(allow mach-lookup)",
                 "(allow ipc-posix*)", "(allow signal)", "(allow file-read-metadata)", "(allow file-ioctl)",
                 "(allow file-read*" + "".join('\n  (literal "%s")' % p for p in sorted(ancestors))
                 + "".join('\n  (subpath "%s")' % p for p in reads) + ")",
                 '(allow file-write* (subpath "%s") (subpath "%s") (literal "%s/.package-cache")'
                 ' (literal "/dev/null") (literal "/dev/tty") (subpath "/dev/fd"))'
                 % (self.out, self.home, self.cargo_home),
                 "(deny network*)", ""]
        return "\n".join(lines)

    def wrap(self, command):
        return ["sandbox-exec", "-p", self.profile()] + command


def sandbox_for(plan, out, scratch, cargo_home, rustup_home):
    if sys.platform == "darwin":
        if not shutil.which("sandbox-exec"):
            raise Miss("sandbox-exec not found")
        return SandboxExec(plan, out, scratch, cargo_home, rustup_home)
    if not shutil.which("bwrap"):
        raise Miss("bwrap (bubblewrap) not found; install it to build hermetically")
    return Bubblewrap(plan, out, scratch, cargo_home, rustup_home)


# ---- driver ----------------------------------------------------------------

def unit_slug(unit):
    return "-".join(filter(None, [unit.package, unit.kind, unit.name if unit.kind == "bin" else None,
                                  unit.profile, unit.target]))


def build(plan, sandbox, out):
    env = plan.environment(out, sandbox.home, sandbox.cargo_home, sandbox.rustup_home)
    (sandbox.home / "tmp").mkdir(parents=True, exist_ok=True)
    out.mkdir(parents=True, exist_ok=True)
    command = plan.unit.cargo_args()
    command[command.index("--manifest-path") + 1] = str(plan.manifest)
    command += ["--offline", "--locked"]
    started = time.time()
    done = subprocess.run(sandbox.wrap(command), cwd=str(plan.manifest.parent), env=env, text=True,
                          stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    seconds = time.time() - started
    if done.returncode:
        raise Miss("hermetic build failed (%d) after %.0fs: %s" % (done.returncode, seconds,
                                                                     done.stderr.strip()[-3000:]))
    outputs = {}
    for line in done.stdout.splitlines():
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact":
            continue
        package = message["package_id"]
        if BK.parse_package_id(package)[1] != plan.unit.package:
            continue
        for path in ([message["executable"]] if message.get("executable") else []) + message.get("filenames", []):
            outputs[BK.relative(path, out) or path] = BK.sha256_file(path)
    if not outputs:
        raise Miss("the build produced no artifact for %s" % plan.unit.package)
    return outputs, seconds


def diagnose(plan, out, env):
    """List what rustc read (dep-info) and flag reads outside the exposed set."""
    unit = BK.Unit(plan.manifest, plan.unit.package, plan.unit.kind, plan.unit.name, plan.unit.target,
                   plan.unit.profile, plan.unit.features, plan.unit.no_default_features)
    discovered = BK.discover_inputs(unit, plan.src, dict(env, PATH=os.environ.get("PATH", "")))
    home = BK.cargo_home(env)
    toolchain = BK.sysroot(unit, env)
    exposed = plan.exposed()
    outside = set()
    repo_reads = set()
    for path, package_id in discovered["raw_files"]:
        kind, normalized = BK.classify(path, plan.src, home, toolchain, discovered["generated_roots"])
        if kind == "repo":
            repo_reads.add(normalized)
            if not any(normalized == rel or normalized.startswith(rel + "/") for rel in exposed):
                outside.add(normalized)
        elif kind == "foreign":
            outside.add(path)
    print("diagnose: %d repository files read, %d generated, %d env names" % (
        len(repo_reads), sum(1 for p, _ in discovered["raw_files"]
                             if BK.classify(p, plan.src, home, toolchain, discovered["generated_roots"])[0] == "generated"),
        len(discovered["env"])))
    if outside:
        raise Miss("compiler read outside the exposed set: " + ", ".join(sorted(outside)))


def compute(options, environ):
    unit = BK.Unit(options.manifest_path, options.package, options.kind, options.name, options.target,
                   options.profile, [f for f in options.features.split(",") if f], options.no_default_features)
    repo = BK.repo_root(unit.manifest.parent)
    if sh(["git", "status", "--porcelain"], cwd=repo).stdout.strip() and not options.allow_dirty:
        raise Miss("worktree has uncommitted changes; a hermetic build keys HEAD exactly (commit, or --allow-dirty)")
    commit = sh(["git", "rev-parse", "HEAD"], cwd=repo).stdout.strip()
    work = Path(os.path.realpath(options.work if options.work else tempfile.mkdtemp(prefix="hermetic-")))
    src = work / "src"
    if src.exists():
        shutil.rmtree(src)
    clean_checkout(repo, commit, src)
    cargo_home = BK.cargo_home(environ)
    rustup_home = Path(os.path.realpath(environ.get("RUSTUP_HOME") or Path.home() / ".rustup"))
    if not options.key_only:
        # Fetch outside the sandbox so the build itself runs offline.
        sh(["cargo", "fetch", "--locked", "--manifest-path", str(src / BK.relative(unit.manifest, repo))],
           env=environ)
    plan = Plan(unit, repo, commit, src, options.edges, environ)
    out = Path(options.out).resolve() if options.out else repo / "target-hermetic" / unit_slug(unit)
    sandbox = sandbox_for(plan, out, work, cargo_home, rustup_home)
    document = plan.document(sandbox.identity())
    key = hashlib.sha256(BK.canonical(document).encode("utf-8")).hexdigest()
    return plan, sandbox, document, key, out, work, commit


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manifest-path", required=True, help="workspace root Cargo.toml of the unit")
    parser.add_argument("--package", required=True)
    parser.add_argument("--kind", choices=BK.KINDS, required=True)
    parser.add_argument("--name", help="binary name (bin units; default: package name)")
    parser.add_argument("--target", help="target triple (default: host)")
    parser.add_argument("--profile", help="cargo profile (default: dev, or test for test units)")
    parser.add_argument("--features", default="", help="comma-separated features")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--edges", default="scripts/build-key-edges.json", help="edges map, repository-relative")
    parser.add_argument("--out", help="target dir for the build (default: <repo>/target-hermetic/<unit>)")
    parser.add_argument("--work", help="directory for the clean checkout and HOME (default: temp)")
    parser.add_argument("--keep-work", action="store_true")
    parser.add_argument("--allow-dirty", action="store_true", help="key HEAD even if the worktree is dirty")
    parser.add_argument("--key-only", action="store_true", help="compute the key without building")
    parser.add_argument("--diagnose", action="store_true", help="after building, list dep-info reads")
    parser.add_argument("--receipt", help="write key, document and output digests as JSON")
    parser.add_argument("--json", help="write the canonical key document")
    options = parser.parse_args(argv)
    work = None
    try:
        plan, sandbox, document, key, out, work, commit = compute(options, dict(os.environ))
        receipt = {"key": key, "commit": commit, "unit": document["unit"], "document": document}
        if options.json:
            Path(options.json).write_text(BK.canonical(document) + "\n")
        if options.key_only:
            print("key=%s trees=%d files=%d lock=%d packages=%d sandbox=%s" % (
                key, len(document["trees"]), len(document["files"]), len(document["lock"]),
                len(document["closure"]), sandbox.kind))
        else:
            outputs, seconds = build(plan, sandbox, out)
            receipt.update({"outputs": outputs, "seconds": round(seconds, 1)})
            print("key=%s trees=%d files=%d lock=%d packages=%d sandbox=%s seconds=%.0f" % (
                key, len(document["trees"]), len(document["files"]), len(document["lock"]),
                len(document["closure"]), sandbox.kind, seconds))
            for path, digest in sorted(outputs.items()):
                print("output %s %s" % (digest, path))
            if options.diagnose:
                diagnose(plan, out, plan.environment(out, sandbox.home, sandbox.cargo_home, sandbox.rustup_home))
        if options.receipt:
            Path(options.receipt).write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
    except Miss as miss:
        print("key=None reason=%s" % miss)
        return BK.EXIT_MISS
    finally:
        if work is not None and not options.keep_work and not options.work:
            shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
