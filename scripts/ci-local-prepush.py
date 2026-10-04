#!/usr/bin/env python3
"""Check the current worktree and the exact object supplied by Git pre-push."""
import fcntl
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import signal
import subprocess
import sys


class GateError(Exception):
    pass


INTERRUPTS = {signal.SIGINT, signal.SIGTERM, signal.SIGHUP}
GIT_LOCAL_ENV_VARS = ()


def run(args, cwd, capture=False, lease=None):
    if lease and args[0] == "cargo":
        disk_reserve(cwd)
        build = Path(os.environ["CARGO_BUILD_BUILD_DIR"])
        disk_reserve(next(path for path in (build, *build.parents) if path.exists()))
    print("+ " + shlex.join(str(arg) for arg in args), flush=True)
    environment = os.environ.copy()
    if args[0] == "cargo":
        target = str(Path(cwd).resolve() / "target")
        environment["CARGO_TARGET_DIR"] = target
        environment["CARGO_BUILD_TARGET_DIR"] = target
    if args[0] != "git":
        for name in GIT_LOCAL_ENV_VARS:
            environment.pop(name, None)
    # Own the child before a pending signal can raise in the parent. This gate
    # is single-threaded; the child restores the caller's mask before exec.
    prior_mask = signal.pthread_sigmask(signal.SIG_BLOCK, INTERRUPTS)
    process = None
    completed = False
    try:
        process = subprocess.Popen(
            args, cwd=cwd, text=True, stdout=subprocess.PIPE if capture else None,
            env=environment, close_fds=True, start_new_session=True,
            preexec_fn=lambda: signal.pthread_sigmask(signal.SIG_SETMASK, prior_mask),
        )
        signal.pthread_sigmask(signal.SIG_SETMASK, prior_mask)
        output, _ = process.communicate()
        completed = True
    except BaseException:
        signal.pthread_sigmask(signal.SIG_BLOCK, INTERRUPTS)
        raise
    finally:
        signal.pthread_sigmask(signal.SIG_BLOCK, INTERRUPTS)
        try:
            if process is not None and not completed:
                try:
                    try:
                        os.killpg(process.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        pass
                finally:
                    # Cargo can exit before a rustc/helper descendant.
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait()
                    if process.stdout:
                        process.stdout.close()
        finally:
            signal.pthread_sigmask(signal.SIG_SETMASK, prior_mask)
    if process.returncode:
        if capture and output:
            print(output, end="", flush=True)
        raise GateError("command failed: " + shlex.join(str(arg) for arg in args))
    return output or ""


def git(root, *args):
    return run(["git", *args], root, capture=True).strip()


def paths_from_git(root, *args):
    return [path for path in run(["git", *args], root, capture=True).split("\0") if path]


def snapshot(root):
    flagged = [entry for entry in paths_from_git(root, "ls-files", "-v", "-z")
               if entry[0] == "S" or entry[0].islower()]
    if flagged:
        raise GateError("clear assume-unchanged and skip-worktree index flags before this gate")
    if git(root, "status", "--porcelain=v1", "-z", "--untracked-files=all"):
        raise GateError("commit or preserve the working tree changes before this gate")
    branch = git(root, "symbolic-ref", "--quiet", "HEAD")
    return branch, git(root, "rev-parse", "HEAD"), git(root, "rev-parse", "HEAD^{tree}")


def push_candidate(lines, candidate):
    updates = [line.split() for line in lines if line.strip()]
    if len(updates) != 1 or len(updates[0]) != 4:
        raise GateError("push one current branch at a time; deletions and tags need their own approval")
    local_ref, local_oid, remote_ref, _ = updates[0]
    branch, commit, _ = candidate
    if local_ref != branch or local_oid != commit or not remote_ref.startswith("refs/heads/"):
        raise GateError("the pushed branch and object must match this worktree's current HEAD")


def current_develop(root, commit):
    git(root, "fetch", "--no-tags", "origin", "+refs/heads/develop:refs/remotes/origin/develop")
    develop = git(root, "rev-parse", "refs/remotes/origin/develop")
    if subprocess.run(["git", "merge-base", "--is-ancestor", develop, commit], cwd=root).returncode:
        raise GateError("merge current origin/develop before checking and pushing; the hook keeps HEAD fixed")
    return develop


def disk_reserve(path):
    disk = shutil.disk_usage(path)
    if disk.free * 100 < disk.total * 15:
        raise GateError("restore the 15% disk reserve before local Cargo checks")


def acquire_lease(common, candidate):
    # Keep one inode after release so every worktree/operator locks the same file.
    lease = (common / "local-ai-heavy-build.lock").open("a+")
    try:
        fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        lease.seek(0)
        owner = lease.read().strip()
        lease.close()
        raise GateError("local heavy build lease is busy" + (": " + owner if owner else ""))
    lease.seek(0)
    lease.truncate()
    json.dump({"pid": os.getpid(), "branch": candidate[0], "commit": candidate[1]}, lease)
    lease.flush()
    return lease


def metadata(root, manifest, lease):
    value = json.loads(run([
        "cargo", "metadata", "--no-deps", "--offline", "--format-version", "1",
        "--manifest-path", str(manifest),
    ], manifest.parent, capture=True, lease=lease))
    members = set(value["workspace_members"])
    return Path(value["workspace_root"]), [p for p in value["packages"] if p["id"] in members]


def clean_repository_packages(root, workspace, lease, release=False):
    # A shared build dir can treat older worktree sources as fresh. Refresh
    # every resolved repository package while retaining external dependencies.
    value = json.loads(run([
        "cargo", "metadata", "--locked", "--offline", "--format-version", "1",
        "--manifest-path", str(workspace / "Cargo.toml"),
    ], workspace, capture=True, lease=lease))
    local = [package for package in value["packages"]
             if package.get("source") is None and
             Path(package["manifest_path"]).resolve().is_relative_to(root.resolve())]
    foreign = [package for package in value["packages"] if package not in local]
    names = {package["name"] for package in local}
    if not names:
        raise GateError("repository package clean has an empty scope: " + str(workspace))
    # Cargo clean removes artifacts by package and crate-name globs, including
    # all hashes. A qualified package ID cannot narrow those removal patterns.
    def target_names(packages):
        return {target["name"].replace("-", "_") for package in packages
                for target in package["targets"] if "custom-build" not in target["kind"]}
    collisions = names.intersection(package["name"] for package in foreign)
    collisions.update(target_names(local).intersection(target_names(foreign)))
    if collisions:
        raise GateError("repository package clean collides with external artifact names: " +
                        ", ".join(sorted(collisions)))
    args = ["cargo", "clean", "--locked", "--offline"]
    if release:
        args.append("--release")
    for name in sorted(names):
        args.extend(["-p", name])
    run(args, workspace, lease=lease)


def referenced_inputs(root, source, folder, files, directories):
    text = source.read_text()
    quoted = r'"((?:\\.|[^"\\])*)"'
    if source.suffix == ".rs":
        # Lifetimes are identifiers; character tokens can contain a quote.
        # Consume chars separately so only double-quoted strings supply paths.
        character = r"'(?:\\.|[^'\\\r\n])'"
        literals = [match.group(1) for match in re.finditer(character + "|" + quoted, text)
                    if match.group(1) is not None]
    else:
        single = r"'((?:\\[^\r\n]|[^'\\\r\n])*)'"
        literals = [double or single for double, single in re.findall(quoted + "|" + single, text)]
    literals += re.findall(r"\bscripts/[\w./-]+", text)
    if source.suffix == ".sh":
        # Shell helpers often join a computed script directory with a literal
        # suffix, for example $(dirname "${BASH_SOURCE[0]}")/helper.py.
        literals += [suffix.lstrip("/") for suffix in
                     re.findall(r"/(?:[\w.-]+/)*[\w.-]+\.(?:sh|py|mjs|js)\b", text)]
    found = set()
    for literal in literals:
        if not ("/" in literal or "." in literal) or any(char.isspace() for char in literal):
            continue
        if literal.startswith("/../"):
            literal = literal.lstrip("/")  # concat!(env!("CARGO_MANIFEST_DIR"), "/../../../...")
        if Path(literal).is_absolute() or "$" in literal or "{" in literal:
            continue
        if source.suffix == ".rs" and "/" not in literal and Path(literal).suffix in {".sh", ".py", ".mjs", ".js"}:
            # Chained joins can split the repository scripts directory from
            # its filename. Match that literal name, rather than every script.
            found.update(path for path in files if path.name == literal and path.is_relative_to(root / "scripts"))
        for base in (root, source.parent, folder):
            candidate = Path(os.path.abspath(base / literal))
            if candidate in files:
                found.add(candidate)
            elif candidate in directories:
                found.update(path for path in files if path.is_relative_to(candidate))
    return found


def product_input(path):
    parts = Path(path).parts
    if {"templates", "fixtures"}.intersection(parts) or path in {"components.json", "model-catalog.json"}:
        return True
    capsule = parts[0] == "capsules" or parts[:2] == ("elastos", "capsules")
    return capsule and (Path(path).name == "capsule.json" or "browser" in parts or
                        Path(path).suffix in {".json", ".wasm", ".html", ".js", ".mjs", ".css", ".svg", ".png"})


def external_input_owners(root, packages, paths):
    # Literal file/directory references also cover data read at test execution
    # and Runtime-launched scripts. Generic CI tooling keeps its own checks.
    tooling = {".githooks/pre-push", "scripts/ci-local-prepush.sh",
               "scripts/ci-local-prepush.py", "scripts/ci-local-prepush-test.py"}
    external = {root / path for path in paths if path not in tooling}
    if not external:
        return set()
    files = {root / path for path in paths_from_git(root, "ls-files", "-z")} | external
    # A bare "scripts" or "capsules" string cannot identify every descendant.
    directories = {parent for path in files for parent in path.parents
                   if parent.is_relative_to(root) and len(parent.relative_to(root).parts) >= 2}
    owners = set()
    matched = set()
    for package in packages:
        folder = Path(package["manifest_path"]).parent
        pending = [path for path in files if path.suffix == ".rs" and path.is_relative_to(folder)]
        visited = set()
        while pending:
            source = pending.pop()
            if source in visited or not source.is_file():
                continue
            visited.add(source)
            references = referenced_inputs(root, source, folder, files, directories)
            pending.extend(path for path in references if path.suffix in {".sh", ".py", ".mjs", ".js"}
                           and {"scripts", "tools"}.intersection(path.relative_to(root).parts))
            for changed in external:
                parts = changed.relative_to(root).parts
                template = root.joinpath(*parts[:3]) if parts[:2] == ("templates", "capsules") else None
                if changed in references or (template and any(path.is_relative_to(template) for path in references)):
                    owners.add(folder.resolve())
                    matched.add(changed)
    # Each uncertain product input widens scope, including mixed known/unknown inputs.
    if any(product_input(path.relative_to(root).as_posix()) for path in external - matched):
        owners.update(Path(package["manifest_path"]).parent.resolve() for package in packages)
    return owners


def touched_workspaces(root, paths, lease):
    workspaces = {}
    runtime, runtime_packages = metadata(root, root / "elastos/Cargo.toml", lease)
    workspaces[runtime] = runtime_packages
    manifests = {root / path for path in paths_from_git(root, "ls-files", "-z", "*Cargo.toml")}
    discoverable = {manifest for manifest in manifests
                    if not {"templates", "fixtures"}.intersection(manifest.relative_to(root).parts)}
    broad = any(path in {"rust-toolchain.toml", ".cargo/config.toml"} for path in paths)
    runtime_input = broad or any(path in {"elastos/Cargo.toml", "elastos/Cargo.lock"}
                                 or path.startswith(("elastos/.cargo/", "elastos/wit/", "elastos/config/")) for path in paths)
    input_owners = external_input_owners(root, runtime_packages, paths)
    seeds = set(input_owners)
    for package in runtime_packages:
        folder = Path(package["manifest_path"]).parent
        local_input = any((root / path).is_relative_to(folder) and
                          Path(path).suffix not in {".md", ".txt"} for path in paths)
        if runtime_input or local_input:
            seeds.add(folder.resolve())
    # Resolve touched standalone workspaces, including tools and capsules.
    candidates = set()
    for path in paths:
        if path.endswith("/Cargo.toml") and {"templates", "fixtures"}.intersection(Path(path).parts):
            continue
        if path.endswith("/Cargo.toml") and not (root / path).exists():
            raise GateError("removed Rust package needs an explicit acceptance plan: " + path)
        manifest = next((parent / "Cargo.toml" for parent in (root / path).parents
                         if parent / "Cargo.toml" in manifests), None)
        if manifest:
            if manifest in discoverable:
                candidates.add(manifest)
        elif path.endswith("/Cargo.toml") or path.endswith(".rs"):
            raise GateError("changed Rust package was removed or has no manifest: " + path)
    if broad:
        candidates.update(discoverable)
    for manifest in sorted(candidates):
        known = {workspace / "Cargo.toml" for workspace in workspaces}
        known.update(Path(p["manifest_path"]) for ps in workspaces.values() for p in ps)
        if manifest not in known:
            workspace, packages = metadata(root, manifest, lease)
            workspaces[workspace] = packages
    checked = set(workspaces)
    if seeds:
        # --no-deps retains all declared path edges, including dev/build,
        # optional and target-specific dependencies. Discover each workspace
        # once, then follow dependency -> consumer through Runtime and capsules.
        for manifest in sorted(discoverable):
            known = {workspace / "Cargo.toml" for workspace in workspaces}
            known.update(Path(p["manifest_path"]) for ps in workspaces.values() for p in ps)
            if manifest not in known:
                workspace, packages = metadata(root, manifest, lease)
                workspaces[workspace] = packages
        consumers = {}
        for packages in workspaces.values():
            for package in packages:
                consumer = Path(package["manifest_path"]).parent.resolve()
                for dependency in package.get("dependencies", []):
                    if dependency.get("path"):
                        consumers.setdefault(Path(dependency["path"]).resolve(), set()).add(consumer)
        related = set(seeds)
        pending = list(seeds)
        while pending:
            for consumer in consumers.get(pending.pop(), set()) - related:
                related.add(consumer)
                pending.append(consumer)
        checked.update(workspace for workspace, packages in workspaces.items()
                       if any(Path(p["manifest_path"]).parent.resolve() in related for p in packages))
    touched = {}
    for workspace, packages in workspaces.items():
        if workspace not in checked:
            continue
        relative = workspace.relative_to(root).as_posix()
        workspace_change = broad or (workspace == runtime and runtime_input) or any(path in {
            relative + "/Cargo.toml", relative + "/Cargo.lock", relative + "/.cargo/config.toml",
        } for path in paths)
        selected = []
        for package in packages:
            prefix = Path(package["manifest_path"]).parent.relative_to(root).as_posix() + "/"
            if workspace_change or Path(package["manifest_path"]).parent.resolve() in input_owners or any(
                    path.startswith(prefix) for path in paths):
                selected.append(package)
        touched[workspace] = selected
    return touched


def unit_targets(package):
    targets = []
    for target in package["targets"]:
        if not target.get("test", True):
            continue
        if any(kind in {"lib", "rlib", "proc-macro", "cdylib", "dylib", "staticlib"}
               for kind in target["kind"]):
            targets.append((target, ["--lib"]))
        elif "bin" in target["kind"]:
            targets.append((target, ["--bin", target["name"]]))
    return targets


def module_prefix(entry, changed):
    """Prove a conventional file-module path; attributes/includes widen scope."""
    if entry == changed:
        return ""
    try:
        relative = changed.relative_to(entry.parent)
    except ValueError:
        return None
    names = list(relative.parts[:-1])
    if relative.name != "mod.rs":
        names.append(relative.stem)
    current = entry
    for name in names:
        if not current.is_file():
            return None
        source = current.read_text()
        if "/*" in source or re.search(r"#\s*\[\s*path\b|\binclude\s*!", source):
            return None
        declaration = r"(?m)^(?:pub(?:\([^)]*\))?\s+)?mod\s+" + re.escape(name) + r"\s*;"
        if not re.search(declaration, source):
            return None
        folder = current.parent if current.name in {"lib.rs", "main.rs", "mod.rs"} else current.with_suffix("")
        plain, directory = folder / (name + ".rs"), folder / name / "mod.rs"
        if plain.exists() == directory.exists():
            return None
        current = plain if plain.exists() else directory
    return "::".join(names) + "::" if current == changed else None


def unit_plan(root, package, paths):
    targets = unit_targets(package)
    folder = Path(package["manifest_path"]).parent
    changed = [root / path for path in paths if (root / path).is_relative_to(folder)]
    full = [(target, selectors, {""}) for target, selectors in targets]
    if not changed or any(path.suffix != ".rs" or not path.exists() for path in changed):
        return full
    plan = {}
    for path in changed:
        matches = []
        for index, (target, selectors) in enumerate(targets):
            prefix = module_prefix(Path(target["src_path"]), path)
            if prefix is not None:
                matches.append((index, prefix))
        if not matches:
            return full
        for index, prefix in matches:
            plan.setdefault(index, set()).add(prefix)
    return [(targets[index][0], targets[index][1], prefixes) for index, prefixes in sorted(plan.items())]


def passed_tests(output):
    return sum(int(count) for count in re.findall(r"(?m)^test result: ok\. (\d+) passed;", output))


def prepare_process_providers(root, lease):
    providers = {
        "ELASTOS_TEST_PROTECT_PROVIDER_BIN": "protected-content-protect-provider",
        "ELASTOS_TEST_DECRYPT_PROVIDER_BIN": "protected-content-decrypt-provider",
        "ELASTOS_TEST_CUSTODY_PROVIDER_BIN": "custody-provider",
    }
    for name in providers.values():
        clean_repository_packages(root, root / "capsules" / name, lease, release=True)
    for variable, name in providers.items():
        folder = root / "capsules" / name
        run(["cargo", "build", "--release", "--target-dir", "target"], folder, lease=lease)
        binary = folder / "target/release" / name
        if not binary.is_file() or not os.access(binary, os.X_OK):
            raise GateError("candidate process provider is unavailable: " + name)
        os.environ[variable] = str(binary)


def crate_units(root, workspace, package, paths, lease):
    plan = unit_plan(root, package, paths)
    if not plan:
        raise GateError(package["name"] + " has no enabled unit test target")
    prepared = False
    executed = 0
    for target, selectors, prefixes in plan:
        args = ["cargo", "test", "-p", package["name"], *selectors]
        listed = run([*args, "--", "--list"], workspace, capture=True, lease=lease)
        tests = re.findall(r"(?m)^(.+): test$", listed)
        if not tests:
            raise GateError(package["name"] + " has zero unit tests on this host: " + " ".join(selectors))
        # A source module can use tests in another module. Fall back to all
        # tests in that target when a proven file path has no matching tests.
        if any(not any(name.startswith(prefix) for name in tests) for prefix in prefixes):
            prefixes = {""}
        prefixes = {prefix for prefix in prefixes
                    if not any(prefix != parent and prefix.startswith(parent) for parent in prefixes)}
        selected = [name for name in tests if any(name.startswith(prefix) for prefix in prefixes)]
        if package["name"] == "elastos-server" and not prepared and any(
                name.startswith(("protected_content_runtime::", "server_infra::")) for name in selected):
            prepare_process_providers(root, lease)
            prepared = True
        for prefix in sorted(prefixes):
            # libtest accepts multiple full names after --. Exact names keep
            # runtime:: from also selecting protected_content_runtime::.
            filters = ["--exact", *[name for name in tests if name.startswith(prefix)]] if prefix else []
            output = run([*args, "--", "--nocapture", *filters],
                         workspace, capture=True, lease=lease)
            print(output, end="", flush=True)
            count = passed_tests(output)
            if count == 0:
                raise GateError(package["name"] + " executed zero passing unit tests")
            executed += count
    if executed == 0:
        raise GateError(package["name"] + " has zero unit tests on this host")


def gates(root, paths, lease):
    run(["git", "diff", "--check", "origin/develop...HEAD"], root)
    run(["node", "scripts/check-product-data.mjs"], root)
    run(["node", "--test", "scripts/check-product-data.test.mjs"], root)
    if any(path in {".githooks/pre-push", "scripts/ci-local-prepush.sh",
                    "scripts/ci-local-prepush.py", "scripts/ci-local-prepush-test.py"}
           for path in paths):
        run(["python3", "scripts/ci-local-prepush-test.py"], root)
    workspaces = touched_workspaces(root, paths, lease)
    formats = {root / "elastos", root / "capsules/chain-provider", *workspaces}
    for workspace in sorted(formats):
        run(["cargo", "fmt", "--all", "--", "--check"], workspace, lease=lease)
    for workspace in sorted(workspaces):
        clean_repository_packages(root, workspace, lease)
    for workspace, packages in sorted(workspaces.items()):
        run(["cargo", "check", "--workspace", "--all-targets"], workspace, lease=lease)
        if packages:
            args = ["cargo", "clippy", "--all-targets"]
            for package in packages:
                args.extend(["-p", package["name"]])
            run([*args, "--", "-D", "warnings"], workspace, lease=lease)
        for package in packages:
            crate_units(root, workspace, package, paths, lease)


def interrupted(signum, frame):
    # Block repeats before raising, including the transition into run cleanup.
    signal.pthread_sigmask(signal.SIG_BLOCK, INTERRUPTS)
    raise KeyboardInterrupt("received signal " + str(signum))


def main():
    global GIT_LOCAL_ENV_VARS
    if len(sys.argv) not in {1, 3}:
        raise GateError("use this gate directly, or pass Git's remote name and URL")
    root = Path(git(Path.cwd(), "rev-parse", "--show-toplevel"))
    GIT_LOCAL_ENV_VARS = tuple(git(root, "rev-parse", "--local-env-vars").splitlines())
    # The gate prepares candidate provider paths itself; inherited binaries
    # have no source receipt and cannot qualify a test input.
    for name in tuple(os.environ):
        if name.startswith("ELASTOS_TEST_") and name.endswith("_BIN"):
            os.environ.pop(name)
    candidate = snapshot(root)
    if len(sys.argv) == 3:
        push_candidate(sys.stdin, candidate)
    develop = current_develop(root, candidate[1])
    if snapshot(root) != candidate:
        raise GateError("source or HEAD changed while fetching develop")
    print("source={} tree={} develop={}".format(candidate[1], candidate[2], develop), flush=True)
    print("Operator gate: reproduce an unclear failed Mac install/update/Home step locally; "
          "review a large diff with Opus before long Mac CI.", flush=True)
    common = Path(git(root, "rev-parse", "--path-format=absolute", "--git-common-dir"))
    os.environ.setdefault("CARGO_BUILD_BUILD_DIR", str(common.parent / "target-build"))
    build_dir = Path(os.environ["CARGO_BUILD_BUILD_DIR"]).expanduser().resolve()
    os.environ["CARGO_BUILD_BUILD_DIR"] = str(build_dir)
    disk_reserve(root)
    disk_reserve(next(path for path in (build_dir, *build_dir.parents) if path.exists()))
    with acquire_lease(common, candidate) as lease:
        paths = paths_from_git(root, "diff", "--name-only", "-z", "--no-renames", develop + "...HEAD")
        gates(root, paths, lease)
        if snapshot(root) != candidate:
            raise GateError("source or HEAD changed during checks; check the new candidate")
        if current_develop(root, candidate[1]) != develop:
            raise GateError("develop changed during checks; check the new base before pushing")
        if snapshot(root) != candidate:
            raise GateError("source or HEAD changed during final fetch")
    print("Local pre-push gate passed for " + candidate[1], flush=True)


if __name__ == "__main__":
    signal.signal(signal.SIGINT, interrupted)
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGHUP, interrupted)
    try:
        main()
    except (GateError, OSError, ValueError, KeyboardInterrupt) as error:
        print("pre-push stopped: " + str(error), file=sys.stderr)
        sys.exit(1)
