#!/usr/bin/env python3
"""Check CI release/cache decisions without builds, Docker, or publication."""
import base64
import os
import hashlib
import io
import json
from pathlib import Path
import re
import runpy
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import unittest
from unittest import mock


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/ci.yml"
SOURCE = WORKFLOW.read_text()
# CI source-gate steps run sections of this script; it owns the command list.
SOURCE_GATE = (WORKFLOW.parents[2] / "scripts/test-source-gate.sh").read_text()
CACHE_ACTION = WORKFLOW.parents[1] / "actions/rust-compile-cache/action.yml"
LOCAL_CACHE_ACTION = "./.github/actions/rust-compile-cache"
UNIT_CACHE_JOBS = {"lint", "test-elastos", "test-capsules"}
SOURCE_HOME_CACHE_JOBS = {"source-home-linux", "source-home-linux-arm64", "source-home-macos", "update-hop-linux"}
# Read the fixed job/step indentation used here; actionlint checks YAML syntax.
def jobs(source):
    return dict(re.findall(r"(?ms)^  ([\w-]+):\n(.*?)(?=^  [\w-]+:\n|\Z)",
                           source.split("\njobs:\n", 1)[1]))


JOBS = jobs(SOURCE)
CACHE_RE = re.compile(
    r"uses: (?:Swatinem/rust-cache|actions/cache(?:/restore|/save)?)@"
    r"|uses: \./\.github/actions/rust-compile-cache\b"
    r"|^\s+cache(?:-from|-to)?:|type=gha", re.M)


def field(block, name):
    match = re.search(rf"(?m)^\s*{re.escape(name)}: ([^\n]+)$", block)
    if not match:
        raise AssertionError(f"missing {name}")
    return match[1]


def steps(job, workflow_jobs=JOBS):
    block = workflow_jobs[job]
    header = re.search(r"(?m)^    steps:([^\n]*)\n", block)
    if not header:
        raise AssertionError(f"missing steps in {job}")
    suffix = header[1].strip()
    if suffix == "*source_home_linux_steps":
        if job != "source-home-linux-arm64" or not re.search(
                r"(?m)^    steps: &source_home_linux_steps$", workflow_jobs["source-home-linux"]):
            raise AssertionError("Linux steps alias requires its known source-home anchor")
        return steps("source-home-linux", workflow_jobs)
    if suffix and (job != "source-home-linux" or suffix != "&source_home_linux_steps"):
        raise AssertionError(f"unknown steps anchor or alias in {job}")
    return re.split(r"(?m)^      - ", block[header.end():])[1:]


def evaluate(expression, context):
    # Evaluate only this repository's workflow/action expressions, never external input.
    expression = expression.removeprefix("${{").removesuffix("}}").strip()
    expression = expression.replace("&&", " and ").replace("||", " or ")
    expression = re.sub(r"!(?!=)", " not ", expression)
    expression = re.sub(r"(?:github|inputs|env|steps|matrix|needs)\.[\w.-]+",
                        lambda match: repr(context[match[0]]), expression)
    return eval(expression, {"__builtins__": {}},
                {"startsWith": str.startswith, "true": True, "false": False})


def validate_action_pins(source, cache_action):
    commits = {}
    for index, block in enumerate((source, cache_action)):
        uses = re.findall(r"(?m)^\s+(?:- )?uses: (\S+)(.*)$", block)
        if not uses and index == 0:
            raise AssertionError("missing action pins")
        for action, comment in uses:
            if index == 0 and action == LOCAL_CACHE_ACTION:
                continue
            if not re.fullmatch(r"[\w.-]+/[\w./-]+@[0-9a-f]{40}", action):
                raise AssertionError(f"unrecognized local action or mutable action: {action}")
            if not re.fullmatch(r" # \S+", comment):
                raise AssertionError(f"missing version comment: {action}")
            commits.setdefault((action.split("@")[0], comment), set()).add(action.split("@")[1])
    # One version comment names one commit, including nested external actions.
    if any(len(value) > 1 for value in commits.values()):
        raise AssertionError("action pin drifts under the same version label")


def run_compile_cache_fixture(source, download_status=0, server_status=0,
                              platform=("Linux", "X64"), archive=None, checksum=None):
    """Execute the action's shell step as the runner does: no Actions runtime."""
    install = textwrap.dedent(source.split("      run: |\n", 1)[1])
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        shims = root / "shims"
        shims.mkdir()
        for name, body in {
            "curl": 'printf "%s\\n" "$@" > "$DOWNLOAD_LOG"\nwhile [ "$1" != -o ]; do shift; done\nprintf fixture > "$2"\nexit "$DOWNLOAD_STATUS"\n',
            # tar -xzf ARCHIVE -C DIRECTORY MEMBER
            "tar": 'mkdir -p "$4/${5%/*}"\ncp "$SERVER_SHIM" "$4/$5"\n',
        }.items():
            path = shims / name
            path.write_text("#!/bin/bash\n" + body)
            path.chmod(0o700)
        sccache = root / "sccache-shim"
        sccache.write_text('#!/bin/bash\n[ "$*" = --start-server ] || exit 99\nenv > "$SERVER_LOG"\nexit "$SERVER_STATUS"\n')
        sccache.chmod(0o700)
        # Hash for real; the corrupt-archive test checks real verification.
        for name in ("sha256sum", "shasum"):
            path = shims / name
            path.write_text(f"#!{sys.executable}\n" + textwrap.dedent("""\
                import os, pathlib, sys
                pathlib.Path(os.environ['CHECKSUM_LOG']).write_bytes(sys.stdin.buffer.read())
                """))
            path.chmod(0o700)
        env = {**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
               "RUNNER_OS": platform[0], "RUNNER_ARCH": platform[1], "RUNNER_TEMP": str(root),
               "GITHUB_PATH": str(root / "path"), "GITHUB_ENV": str(root / "env"),
               "DOWNLOAD_STATUS": str(download_status), "SERVER_STATUS": str(server_status),
               "SERVER_LOG": str(root / "server"), "SERVER_SHIM": str(sccache),
               "DOWNLOAD_LOG": str(root / "download"),
               "CHECKSUM_LOG": str(root / "checksum")}
        for name in list(env):
            if name.startswith(("ACTIONS_", "SCCACHE_")) or name == "RUSTC_WRAPPER":
                del env[name]
        result = subprocess.run(["bash", "-e", "-c", install], env=env,
                                capture_output=True, text=True, cwd=root, timeout=10)
        if archive is not None:
            assert result.returncode == 0, result.stderr
            assert f"/v0.18.0/{archive}.tar.gz" in (root / "download").read_text()
            assert (root / "checksum").read_text() == f"{checksum}  {root}/rust-compile-cache/{archive}.tar.gz\n"
        exports = dict(line.split("=", 1) for line in (root / "env").read_text().splitlines()) \
            if (root / "env").exists() else {}
        daemon = dict(line.split("=", 1) for line in (root / "server").read_text().splitlines()
                      if "=" in line) if (root / "server").exists() else None
        return result.returncode, exports, daemon


# event, workflow ref, ref type, checkout override, caches
CASES = [
    ("push", "refs/heads/main", "branch", "", True),
    ("push", "refs/heads/develop", "branch", "", True),
    ("push", "refs/heads/v-work", "branch", "", True),
    ("push", "refs/tags/v0.7.1", "tag", "", False),
    ("push", "refs/tags/candidate", "tag", "", False),
    ("pull_request", "refs/pull/1/merge", "branch", "", True),
    ("merge_group", "refs/heads/gh-readonly-queue/develop/pr-1", "branch", "", True),
    ("workflow_dispatch", "refs/heads/main", "branch", "", True),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "", False),
    ("workflow_dispatch", "refs/heads/main", "branch", "v0.7.1", False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/tags/v0.7.1", False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/heads/work", False),
    ("workflow_dispatch", "refs/heads/main", "branch", "a" * 40, False),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "main", False),
]
# Pushes to these branches run every trusted proof; only develop writes caches.
SAVING_REFS = {"refs/heads/develop", "refs/heads/main"}
CACHE_SAVING_REF = "refs/heads/develop"
RUST_CACHE_KEY = "-${{ hashFiles('**/Cargo.lock', 'rust-toolchain.toml') }}"
# rustsec/rustsec release cargo-audit/v0.22.2, x86_64-unknown-linux-musl archive.
AUDIT_ARCHIVE_SHA256 = "7fb9497f8594b389e5fce5ef9b92db08432996895b2e0c5a0167a69ed445c428"
# A restored non-Rust input and the step that checks it against its pinned hash.
RESTORE_VERIFIERS = {
    "restore pinned ARM64 engine": 'build-llama-server-bundle.sh --verify-archive "$RUNNER_TEMP/llama-arm64-bundle"',
    "restore pinned AMD64 engine": 'build-llama-server-bundle.sh --platform linux-amd64 --verify-archive "$RUNNER_TEMP/llama-amd64-bundle"',
    "restore Ubuntu prerequisite archives": "bash scripts/ci-apt-prerequisites.sh",
    "restore verified Kubo inputs": "python3 scripts/ci-kubo-cache.py fetch ",
    "restore the pinned published release": "python3 scripts/update-hop-compare.py fetch-previous-release",
}
NO_CACHE_HIT = {"steps.engine-cache.outputs.cache-hit": "false",
                "steps.previous-release-cache.outputs.cache-hit": "false",
                "steps.kubo-cache.outputs.cache-hit": "false",
                "steps.apt-prerequisites.outputs.changed": "true"}


def job_runs(job, context, workflow_jobs=JOBS):
    guard = re.search(r"(?m)^    if: (.*)$", workflow_jobs[job])
    return evaluate(guard[1], context) if guard else True


def matrix_rows(block):
    rows = re.match(r"(?:          .*\n)+", block.split("        include:\n", 1)[1])[0]
    return [dict(re.findall(r"(\w+): (\S+)", row)) for row in re.split(r"(?m)^          - ", rows)[1:]]


def custody_should_run(source, context, paths):
    workflow_jobs = jobs(source)
    if not job_runs("custody-harness-smoke", context, workflow_jobs):
        return False
    step, = [step for step in steps("custody-harness-smoke", workflow_jobs) if "id: should-run" in step]
    script = textwrap.dedent(step.split("        run: |\n", 1)[1])
    context = {**context, "steps.filter.outputs.custody": paths}
    script = re.sub(r"\$\{\{.*?\}\}", lambda match: str(evaluate(match[0], context)), script)
    with tempfile.TemporaryDirectory() as directory:
        output = Path(directory) / "output"
        subprocess.run(["bash", "-e", "-c", script], check=True,
                       env={**os.environ, "GITHUB_OUTPUT": str(output),
                            "GITHUB_STEP_SUMMARY": str(Path(directory) / "summary")})
        return output.read_text() == "run=true\n"


REPO = WORKFLOW.parents[2]
CANARY = WORKFLOW.with_name("canary-holder-linux.yml")
TRACKED = frozenset(subprocess.check_output(["git", "ls-files"], cwd=REPO, text=True).splitlines())
CALLED_PATH = re.compile(r"\$\{?(?:ROOT|REPO_ROOT)\}?/([\w./-]+)|\$\{?SCRIPT_DIR\}?/([\w./-]+)"
                         r"|(?<![\w./$}-])((?:scripts|deploy|capsules|elastos)/[\w./-]+|components\.json)")
CARGO_WORKSPACE = re.compile(r"--manifest-path ([\w./-]+)/Cargo\.toml|\bcd ([\w./-]+) && cargo\b")
CARGO_PACKAGE = re.compile(r"(?:\s-p|\s--package)[ =]([\w-]+)")
PATH_DEPENDENCY = re.compile(r'(?m)^[\w-]+\s*=\s*\{[^}\n]*\bpath\s*=\s*"([^"]+)"')
# The custody proof driver calls setup-source-home.sh with a mismatched
# ELASTOS_DATA_DIR on purpose; it refuses before it reads any other input.
FILTER_LEAVES = {"custody-harness-smoke": {"scripts/setup-source-home.sh"}}
# The release upstream build loads helper modules and packages every repository
# file its recipes name; those are inputs too, not leaves.
FILTER_FOLLOWS = {"release-upstream-assets": {"scripts/release-upstream-assets.py"}}


def tracked_under(path):
    return {path} if path in TRACKED else {name for name in TRACKED if name.startswith(path.rstrip("/") + "/")}


def cargo_configs(directory):
    """Cargo reads .cargo/config.toml (or .cargo/config) in its directory and every ancestor."""
    parts = Path(directory).parts
    return {name for depth in range(len(parts) + 1) for config in (".cargo/config.toml", ".cargo/config")
            for name in [Path(*parts[:depth], config).as_posix()] if name in TRACKED}


def capsule_inputs(crate):
    """A capsule crate is wholly an input, with every crate it names by path, transitively."""
    found, queue, seen = set(), [crate], set()
    while queue:
        crate = queue.pop()
        if crate in seen:
            continue
        seen.add(crate)
        found |= tracked_under(crate)
        queue += [os.path.normpath(f"{crate}/{dependency}")
                  for dependency in PATH_DEPENDENCY.findall((REPO / crate / "Cargo.toml").read_text())]
    return found


def workspace_member(workspace, package):
    manifest = (REPO / workspace / "Cargo.toml").read_text()
    members = re.findall(r'"([^"]+)"', manifest.split("members", 1)[1].split("]", 1)[0])
    return next(f"{workspace}/{member}" for member in members if re.search(
        r'(?m)^name\s*=\s*"' + re.escape(package) + '"', (REPO / workspace / member / "Cargo.toml").read_text()))


def called_inputs(text, directory):
    """Tracked repository files a script or step names outside comments.

    A Cargo build brings its toolchain pin, the Cargo configuration it reads and
    its workspace manifest and lock; a capsule workspace or a capsule package
    selected with -p is wholly an input, with its path dependencies. Elastos
    crates stay scoped to the modules a filter names."""
    found = set()
    for line in text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        for rooted, local, bare in CALLED_PATH.findall(line):
            path = os.path.normpath(rooted or bare or f"{directory}/{local}")
            if path != ".":
                found |= tracked_under(path)
        for manifest, directory_change in CARGO_WORKSPACE.findall(line):
            workspace = manifest or directory_change
            found |= tracked_under(workspace + "/Cargo.toml") | tracked_under(workspace + "/Cargo.lock")
            found |= cargo_configs(workspace if directory_change else ".")
            if workspace.startswith("capsules/"):
                found |= capsule_inputs(workspace)
            for package in CARGO_PACKAGE.findall(line):
                member = workspace_member(workspace, package)
                if "/capsules/" in "/" + member:
                    found |= capsule_inputs(member)
        if re.search(r"\bcargo (?:build|test|clippy|run)\b", line):
            found |= {"rust-toolchain.toml"} | cargo_configs(".")
    return found


def job_inputs(workflow, source, job, leaves=frozenset(), follows=frozenset()):
    """The workflow file plus every input the job's run steps call, following
    shell scripts and Dockerfiles; Python and Node callees are leaf inputs,
    except followed entry points, whose Python and JSON inputs are followed too."""
    texts = []
    for step in steps(job, jobs(source)):
        if "        run: |\n" in step:
            texts.append(textwrap.dedent(step.split("        run: |\n", 1)[1]))
        else:
            texts += re.findall(r"(?m)^        run: (.+)$", step)
    inputs = {workflow.relative_to(REPO).as_posix()}
    queue = [(text, ".", False) for text in texts]
    while queue:
        text, directory, following = queue.pop()
        for path in sorted(called_inputs(text, directory) - inputs):
            inputs.add(path)
            follow = path in follows or (following and path.endswith((".py", ".json")))
            if (path.endswith(".sh") or Path(path).name.startswith("Dockerfile") or follow) and path not in leaves:
                queue.append(((REPO / path).read_text(), Path(path).parent.as_posix(), follow))
    return inputs


def glob_regex(pattern):
    return re.compile("".join(".*" if part == "**" else "[^/]*" if part == "*" else re.escape(part)
                              for part in re.split(r"(\*\*|\*)", pattern)) + r"\Z")


def paths_filter(source, job, name):
    step, = [step for step in steps(job, jobs(source)) if "id: filter" in step]
    return re.findall(r"(?m)^              - '([^']+)'$", step.split(f"            {name}:\n", 1)[1].split("\n\n", 1)[0])


def custody_filter(source):
    return paths_filter(source, "custody-harness-smoke", "custody")


def upstream_filter(source):
    return paths_filter(source, "release-upstream-paths", "upstream")


def upstream_assets_run(source, context, changed):
    """The path job's output decides the build job's guard, as Actions evaluates them."""
    workflow_jobs = jobs(source)
    output = field(workflow_jobs["release-upstream-paths"].split("    outputs:\n", 1)[1], "run")
    run = evaluate(output, {**context, "steps.filter.outputs.upstream": changed})
    return job_runs("release-upstream-assets",
                    {**context, "needs.release-upstream-paths.outputs.run": str(run).lower()}, workflow_jobs)


def canary_filter(source):
    block = source.split("\n  pull_request:\n    ", 1)[1].split("\n  workflow_dispatch:", 1)[0]
    return re.findall(r"(?m)^      - (\S+)$", block.split("paths:\n", 1)[1])


def validate_filter_inputs(workflow, source, job, patterns):
    """A path filter skips a required check only when none of its inputs changed."""
    matchers = [glob_regex(pattern) for pattern in patterns]
    missing = sorted(path for path in job_inputs(workflow, source, job, FILTER_LEAVES.get(job, frozenset()),
                                                 FILTER_FOLLOWS.get(job, frozenset()))
                     if not any(matcher.match(path) for matcher in matchers))
    if missing:
        raise AssertionError(f"{job} filter misses inputs: {', '.join(missing)}")


def validate_cache_guards(source):
    workflow_jobs = jobs(source)
    for job in workflow_jobs:
        for step in steps(job, workflow_jobs):
            if not CACHE_RE.search(step):
                continue
            guard = field(step, "if")
            if "env.CI_USE_CACHE == 'true'" not in guard:
                raise AssertionError(f"unguarded cache in {job}")
            for event, ref, ref_type, override, cached in CASES:
                if cached:
                    continue
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override,
                           **NO_CACHE_HIT}
                for name in ("CI_USE_CACHE", "CI_SAVE_CACHE"):
                    context["env." + name] = str(evaluate(field(source, name), context)).lower()
                context["steps.should-run.outputs.run"] = str(custody_should_run(source, context, "true")).lower()
                if job_runs(job, context, workflow_jobs) and evaluate(guard, context):
                    raise AssertionError(f"cache guard permits uncached build in {job}")


CREDENTIAL_CHECK = "name: Rust compile cache holds no Actions credential\n"


def validate_cache_credential_scope(source, action):
    """The compile-cache daemon runs rustc and proc macros for every later step."""
    # The runner gives its cache credential to Node actions, never to shell steps.
    if re.search(r"(?m)^\s*(?:- )?uses:", action):
        raise AssertionError("cache action may run only shell steps")
    if re.search(r"ACTIONS_|exportVariable|(?i:gha)", action):
        raise AssertionError("cache action names an Actions credential or remote cache")
    if re.search(r"exportVariable\(\s*['\"`]ACTIONS_|^\s+ACTIONS_\w+:|ACTIONS_\w+=\S*\s*>>"
                 r"|SCCACHE_(?:GHA|MULTILEVEL)", source, re.M):
        raise AssertionError("workflow passes an Actions credential or remote cache to later steps")
    workflow_jobs = jobs(source)
    scripts = set()
    for job in workflow_jobs:
        job_steps = steps(job, workflow_jobs)
        setup = [index for index, step in enumerate(job_steps) if f"uses: {LOCAL_CACHE_ACTION}" in step]
        if not setup:
            continue
        checks = [index for index, step in enumerate(job_steps) if step.startswith(CREDENTIAL_CHECK)]
        if len(checks) != 1 or checks[0] < setup[0]:
            raise AssertionError(f"{job} lacks one credential check after the cache action")
        check = job_steps[checks[0]]
        if field(check, "if") != "always() && env.RUSTC_WRAPPER == 'sccache'":
            raise AssertionError(f"{job} credential check must run whenever sccache is enabled")
        # Every shell step, and so every compile, precedes the check.
        if any(re.search(r"(?m)^\s+run:", step) for step in job_steps[checks[0] + 1:]):
            raise AssertionError(f"{job} runs shell steps after its credential check")
        body = re.match(r"(?:(?: {10}[^\n]*)?\n)*", check.split("        run: |\n", 1)[1])[0]
        scripts.add(textwrap.dedent(body).strip() + "\n")
    if len(scripts) != 1:
        raise AssertionError("every cache job runs the same credential check")
    return scripts.pop()


def validate_cache_writers(source):
    """Only develop pushes write any shared cache; every other run restores only."""
    if re.search(r"type=gha|cache-to", source):
        raise AssertionError("BuildKit layers bypass pinned-hash checks; no remote layer cache")
    workflow_jobs = jobs(source)
    writers = [(job, step) for job in workflow_jobs for step in steps(job, workflow_jobs)
               if "actions/cache/save@" in step or "Swatinem/rust-cache@" in step]
    if not writers:
        raise AssertionError("missing cache writers")
    for event, ref, ref_type, override, _ in [*CASES, ("push", "refs/heads/feature", "branch", "", True)]:
        context = {"github.event_name": event, "github.ref": ref,
                   "github.ref_type": ref_type, "inputs.ref": override,
                   "env.CI_USE_CACHE": "true", "steps.should-run.outputs.run": "true",
                   "steps.engine-cache.outputs.cache-hit": "false", "steps.kubo-cache.outputs.cache-hit": "false",
                   "steps.previous-release-cache.outputs.cache-hit": "false",
                   "steps.apt-prerequisites.outputs.changed": "true"}
        context["env.CI_SAVE_CACHE"] = str(evaluate(field(source, "CI_SAVE_CACHE"), context)).lower()
        trusted = event == "push" and ref == CACHE_SAVING_REF
        if (context["env.CI_SAVE_CACHE"] == "true") != trusted:
            raise AssertionError(f"CI_SAVE_CACHE on {event} {ref}")
        for job, step in writers:
            guard = field(step, "save-if" if "Swatinem/rust-cache@" in step else "if")
            if evaluate(guard, context) and not trusted:
                raise AssertionError(f"{job} saves a cache on {event} {ref}")


def validate_rust_cache_keys(source):
    """A Rust cache restores only under the same lockfiles and toolchain, even by prefix."""
    workflow_jobs = jobs(source)
    caches = [(job, step) for job in workflow_jobs for step in steps(job, workflow_jobs)
              if "Swatinem/rust-cache@" in step]
    if not caches:
        raise AssertionError("missing Rust caches")
    for job, step in caches:
        # rust-cache's fallback restore key is prefix-key, job, OS and the rustc
        # environment hash; its lockfile hash covers only the named workspace.
        if not field(step, "prefix-key").endswith(RUST_CACHE_KEY):
            raise AssertionError(f"{job} Rust cache prefix lacks every lockfile and the toolchain")
        if re.search(r"(?m)^\s+(?:add-rust-environment-hash-key|add-job-id-key|key|shared-key):", step):
            raise AssertionError(f"{job} Rust cache key must keep its compiler and job parts")


def validate_restores_are_verified(source):
    """Every restored non-Rust input passes its pinned-hash check before any use."""
    workflow_jobs = jobs(source)
    for job in workflow_jobs:
        job_steps = steps(job, workflow_jobs)
        for index, step in enumerate(job_steps):
            if "actions/cache/restore@" not in step:
                continue
            name = field(step, "name")
            if name not in RESTORE_VERIFIERS:
                raise AssertionError(f"{job} restores {name} without a known verifier")
            step_id = field(step, "id") if re.search(r"(?m)^\s+id:", step) else None
            for later in job_steps[index + 1:]:
                if RESTORE_VERIFIERS[name] in later:
                    if re.search(r"(?m)^        if:", later):
                        raise AssertionError(f"{job} may skip verifying {name}")
                    break
                # Only a cache-miss build or a save may run before the check.
                miss = step_id and field(later, "if").startswith(f"steps.{step_id}.outputs.cache-hit != 'true'") \
                    if re.search(r"(?m)^        if:", later) else False
                if not (miss or "actions/cache/save@" in later):
                    raise AssertionError(f"{job} uses {name} before verifying it")
            else:
                raise AssertionError(f"{job} never verifies {name}")


def validate_jetson_package_lifecycle(source):
    job_steps = steps("source-home-linux-arm64", jobs(source))
    package, = [step for step in job_steps if step.startswith("name: build and package release binaries\n")]
    verify, = [step for step in job_steps if step.startswith("name: verify Jetson release compatibility\n")]
    if job_steps.index(package) >= job_steps.index(verify):
        raise AssertionError("Jetson verification requires an earlier package step")
    if "scripts/package-release-binaries.sh" not in package:
        raise AssertionError("Jetson verification requires the package producer")
    # The packaged binaries come from the committed lockfile, never a fresh resolution.
    if any("--locked" not in build for build in re.findall(r"cargo build[^\n]*", package)):
        raise AssertionError("release package builds require --locked")
    for event, ref, ref_type, override, _ in CASES:
        for runner in ("ubuntu-24.04", "ubuntu-22.04-arm"):
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override, "matrix.os": runner}
            if evaluate(field(verify, "if"), context) and not evaluate(field(package, "if"), context):
                raise AssertionError(f"Jetson verification lacks its package on {event} {runner}")


def validate_dependency_audit(source):
    """Every tracked lockfile meets the current RustSec database on every event."""
    workflow_jobs = jobs(source)
    if "cargo-audit" not in workflow_jobs:
        raise AssertionError("missing cargo-audit job")
    job = workflow_jobs["cargo-audit"]
    if re.search(r"(?m)^\s+(?:if|needs|continue-on-error):", job):
        raise AssertionError("cargo-audit runs and fails on every event")
    if CACHE_RE.search(job):
        raise AssertionError("cargo-audit restores nothing; its database must be fresh")
    job_steps = steps("cargo-audit", workflow_jobs)
    install, = [step for step in job_steps if step.startswith("name: install checksummed cargo-audit\n")]
    audit, = [step for step in job_steps if step.startswith("name: audit every Cargo.lock\n")]
    if job_steps.index(install) >= job_steps.index(audit):
        raise AssertionError("cargo-audit is installed before the audit")
    digest = '| sha256sum -c -'
    if not (f"{AUDIT_ARCHIVE_SHA256}  " in install and digest in install
            and install.index(digest) < install.index("tar -xzf") < install.index("GITHUB_PATH")):
        raise AssertionError("cargo-audit archive is checked against its pinned digest before use")
    # Reviewed exceptions live only in elastos/.cargo/audit.toml, read from this directory.
    if field(audit, "working-directory") != "elastos" or re.search(r"--ignore\b|audit\.toml|CARGO_HOME", audit):
        raise AssertionError("cargo-audit uses only the reviewed exceptions in elastos/.cargo/audit.toml")
    script = textwrap.dedent(audit.split("        run: |\n", 1)[1])
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory) / "repo"
        locks = ["Cargo.lock", "capsules/one/Cargo.lock", "elastos/Cargo.lock", "examples/two/Cargo.lock"]
        for lock in locks:
            (root / lock).parent.mkdir(parents=True, exist_ok=True)
            (root / lock).write_text("")
        (root / "untracked").mkdir()
        (root / "untracked/Cargo.lock").write_text("")
        subprocess.run(["git", "init", "-q", str(root)], check=True)
        subprocess.run(["git", "-C", str(root), "add", *locks], check=True)
        tools = Path(directory) / "bin"
        tools.mkdir()
        (tools / "cargo-audit").write_text(
            '#!/bin/sh\nprintf \'%s|%s\\n\' "$(pwd -P)" "$*" >> "$AUDIT_LOG"\n'
            'case " $* " in *" ../$AUDIT_FAIL "*) exit 1 ;; esac\n')
        (tools / "cargo-audit").chmod(0o755)
        for failing in (None, *locks):
            log = Path(directory) / "audit.log"
            log.write_text("")
            env = {**os.environ, "PATH": f"{tools}{os.pathsep}{os.environ['PATH']}",
                   "RUNNER_TEMP": directory, "AUDIT_LOG": str(log), "AUDIT_FAIL": failing or "none"}
            result = subprocess.run(["/bin/bash", "-c", script], cwd=root / "elastos", env=env,
                                    capture_output=True, text=True, timeout=30)
            calls = [line.split("|", 1) for line in log.read_text().splitlines()]
            if sorted(args.split("--file ../", 1)[-1] for _, args in calls) != sorted(locks):
                raise AssertionError(f"cargo-audit checks every tracked Cargo.lock (failing {failing})")
            if {cwd for cwd, _ in calls} != {str((root / "elastos").resolve())}:
                raise AssertionError("cargo-audit runs where elastos/.cargo/audit.toml applies")
            if any("--no-fetch" in args for _, args in calls):
                raise AssertionError("cargo-audit retains online yanked checks for every lockfile")
            if (result.returncode != 0) != (failing is not None):
                raise AssertionError(f"cargo-audit result ignores {failing or 'a clean run'}")
        # A lockfile listing that prints part of the list and then fails must fail the job.
        broken = Path(directory) / "broken-git"
        broken.mkdir()
        (broken / "git").write_text('#!/bin/sh\necho Cargo.lock\nexit 2\n')
        (broken / "git").chmod(0o755)
        log = Path(directory) / "audit.log"
        log.write_text("")
        env = {**os.environ, "PATH": f"{broken}{os.pathsep}{tools}{os.pathsep}{os.environ['PATH']}",
               "RUNNER_TEMP": directory, "AUDIT_LOG": str(log), "AUDIT_FAIL": "none"}
        result = subprocess.run(["/bin/bash", "-c", script], cwd=root / "elastos", env=env,
                                capture_output=True, text=True, timeout=30)
        if result.returncode == 0:
            raise AssertionError("cargo-audit fails when the Cargo.lock listing fails")


def validate_journey_builds(source, release_source):
    overrides = {"CARGO_PROFILE_RELEASE_LTO": "false",
                 "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16"}
    for key in overrides:
        if key in release_source or key in source.split("\njobs:\n", 1)[0]:
            raise AssertionError("journey profiles belong only to CI journey jobs")
    for job, block in jobs(source).items():
        if job not in SOURCE_HOME_CACHE_JOBS:
            if any(key in block for key in overrides):
                raise AssertionError(f"journey profile override in {job}")
            continue
        # Runner paths are available in steps, not in job-level env expressions.
        environment = re.search(r"(?ms)^    env:\n(.*?)(?=^    \S|\Z)", block)
        if environment and re.search(r"CARGO_(?:TARGET_DIR|BUILD_BUILD_DIR):|runner\.temp", environment[1]):
            raise AssertionError(f"runner build paths belong in the first step: {job}")
        job_steps = steps(job, jobs(source))
        step_source = "\n".join(job_steps)
        directory_job = "source-home-linux" if job == "source-home-linux-arm64" else job
        paths, = [step for step in job_steps if step.startswith("name: set job build directories\n")]
        if paths != job_steps[0] or re.search(r"(?m)^        if:", paths):
            raise AssertionError(f"build paths must be set by the first unconditional step: {job}")
        for key, value in {"CARGO_TARGET_DIR": "$RUNNER_TEMP/" + directory_job + "-target",
                           "CARGO_BUILD_BUILD_DIR": "$RUNNER_TEMP/" + directory_job + "-build"}.items():
            if f'echo "{key}={value}" >> "$GITHUB_ENV"' not in paths or step_source.count(key + "=") != 1:
                raise AssertionError(f"one shared job build setting required: {job} {key}")
        profile, = [step for step in job_steps if step.startswith("name: use CI journey release profile\n")]
        for key, value in overrides.items():
            if f'echo "{key}={value}" >> "$GITHUB_ENV"' not in profile or step_source.count(key) != 1:
                raise AssertionError(f"missing journey profile setting: {job} {key}")
        for event, ref, ref_type, override, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override}
            expected = event in ("pull_request", "merge_group") or (event == "push" and ref == "refs/heads/develop")
            if evaluate(field(profile, "if"), context) != expected:
                raise AssertionError(f"journey profile changes shipped build: {job} {event} {ref}")
        cache, = [step for step in job_steps if "Swatinem/rust-cache@" in step]
        # Only the action's content-addressed sccache objects join Cargo inputs.
        if field(cache, "cache-targets") != "false" or "source-home-registry" not in cache or \
                re.findall(r"(?m)^\s*cache-directories: (.*)$", cache) != ["${{ env.SCCACHE_DIR }}"]:
            raise AssertionError(f"source journey cache contains build artifacts: {job}")
        fresh, = [step for step in job_steps if 'test ! -e "$directory"' in step]
        if re.search(r"(?m)^        if:", fresh):
            raise AssertionError(f"freshness check must run on every event: {job}")
        for token in ('"$CARGO_TARGET_DIR"', '"$CARGO_BUILD_BUILD_DIR"', 'test ! -L "$directory"'):
            if token not in fresh:
                raise AssertionError(f"fresh job artifacts required: {job}")
        setup, = [step for step in job_steps if step.startswith("name: source-home into isolated")]
        if job_steps.index(fresh) >= job_steps.index(setup):
            raise AssertionError(f"freshness check must precede source-home: {job}")
        if re.search(r"(?:CARGO_TARGET_DIR|CARGO_BUILD_BUILD_DIR|RUSTFLAGS)=|--target-dir",
                     "\n".join(step for step in job_steps if step != paths)):
            raise AssertionError(f"step changes the shared build environment: {job}")


def validate_release_builds_are_hermetic(source):
    workflow_jobs = jobs(source)
    if field(source.split("\njobs:\n", 1)[0], "RUSTC_WRAPPER") != "''":
        raise AssertionError("release jobs must clear any inherited compiler wrapper")
    if re.search(r"(?i)sccache|rust-compile-cache|RUSTC_WRAPPER\s*[=:]\s*['\"]?\w|CI_SAVE_CACHE", source):
        raise AssertionError("shipped release builds use no compiler cache")
    if re.search(r"CARGO_PROFILE_RELEASE_(?:LTO|CODEGEN_UNITS)", source):
        raise AssertionError("release profile retains its owner")
    if re.search(r"uses: actions/cache(?:/save)?@|uses: Swatinem/rust-cache|cache-targets: true|restore-keys:", source):
        raise AssertionError("release workflow permits only exact engine input restore")
    for job in ("mac", "linux"):
        build, = [step for step in steps(job, workflow_jobs)
                  if step.startswith("name: build N, then N+1 with N's support, each in fresh Cargo directories\n")]
        for token in ('for build in "N $INSTALL_VERSION" "N1 $UPDATE_VERSION"',
                      'CARGO_TARGET_DIR="$RUNNER_TEMP/release-cargo-target"',
                      'CARGO_BUILD_BUILD_DIR="$RUNNER_TEMP/release-cargo-build"',
                      '[[ ! -e "$CARGO_TARGET_DIR" && ! -L "$CARGO_TARGET_DIR" && ! -e "$CARGO_BUILD_BUILD_DIR" && ! -L "$CARGO_BUILD_BUILD_DIR" ]]',
                      '[[ "$name" == N ]] || reuse=(--reuse-support "$inputs/N")',
                      'rm -rf "$CARGO_TARGET_DIR" "$CARGO_BUILD_BUILD_DIR"'):
            if token not in build:
                raise AssertionError(f"release builds retain fresh outputs and support parity: {job}")


class ReleasePolicyTests(unittest.TestCase):
    def test_source_home_missing_linux_engine_bundle_refuses_before_tools_or_writes(self):
        for arch, platform, variable in (("x86_64", "linux-amd64", "ELASTOS_LLAMA_AMD64_BUNDLE"),
                                          ("aarch64", "linux-arm64", "ELASTOS_LLAMA_ARM64_BUNDLE")):
            for value in (None, "missing.tar.gz"):
                with self.subTest(platform=platform, bundle=value), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory).resolve()
                    tools = root / "tools"
                    tools.mkdir()
                    (tools / "dirname").symlink_to(shutil.which("dirname"))
                    (tools / "uname").write_text(f'#!/bin/bash\nif [[ "$1" == -s ]]; then echo Linux; else echo {arch}; fi\n')
                    for name in ("cargo", "node", "rustc", "python3"):
                        (tools / name).write_text('#!/bin/bash\necho "$0" >> "$TOOL_LOG"\nexit 91\n')
                    for path in tools.iterdir():
                        if not path.is_symlink():
                            path.chmod(0o700)
                    environment = {**os.environ, "HOME": str(root / "home"), "XDG_DATA_HOME": str(root / "data"),
                                   "PATH": str(tools), "TOOL_LOG": str(root / "tools-called"),
                                   "ELASTOS_CARGO_BIN": str(tools / "cargo"), "ELASTOS_NODE_BIN": str(tools / "node"),
                                   "SETUP_SOURCE_HOME_CONFIG_ONLY": "0", "SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER": "1"}
                    for name in ("ELASTOS_DATA_DIR", "ELASTOS_LLAMA_AMD64_BUNDLE", "ELASTOS_LLAMA_ARM64_BUNDLE"):
                        environment.pop(name, None)
                    if value:
                        environment[variable] = str(root / value)
                    result = subprocess.run(["/bin/bash", str(WORKFLOW.parents[2] / "scripts/setup-source-home.sh")],
                                            env=environment, capture_output=True, text=True, timeout=10)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn(variable + " must name the qualified Linux engine archive", result.stderr)
                    self.assertIn("GGML_OPENMP=OFF", result.stderr)
                    self.assertIn("build-llama-server-bundle.sh --platform " + platform, result.stderr)
                    self.assertIn("SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER=1 scripts/setup-source-home.sh", result.stderr)
                    self.assertNotIn("KeyError", result.stderr)
                    self.assertFalse((root / "tools-called").exists())
                    self.assertFalse((root / "home").exists())
                    self.assertFalse((root / "data").exists())

    def test_source_home_callers_bind_executable_tools_before_isolating_home(self):
        for job in sorted(SOURCE_HOME_CACHE_JOBS):
            setup, = [step for step in steps(job) if step.startswith("name: source-home into isolated")]
            shell = textwrap.dedent(setup.split("        run: |\n", 1)[1])
            prefix = shell.split("scripts/setup-source-home.sh", 1)[0]
            for missing in (None, "node", "turnserver"):
                with self.subTest(job=job, missing=missing), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory).resolve()
                    tools = root / "tools"
                    tools.mkdir()
                    for tool in ("mkdir", "install"):
                        (tools / tool).symlink_to(shutil.which(tool))
                    for tool in ("node", "turnserver", "ffmpeg", "ffprobe"):
                        if tool != missing:
                            (tools / tool).write_text("#!/bin/bash\nexit 0\n")
                            (tools / tool).chmod(0o700)
                    cargo = root / "cargo/bin/cargo"
                    cargo.parent.mkdir(parents=True)
                    cargo.write_text('#!/bin/bash\necho \'{"packages":[{"name":"elastos-server","version":"1.2.3"}]}\'\n')
                    cargo.chmod(0o700)
                    # This runner prefix executes the actual workflow without building Home.
                    # Resolving tools happens while the runner still owns the original HOME/PATH.
                    environment = {**os.environ, "HOME": str(root / "user"), "PATH": str(tools),
                                   "CARGO_HOME": str(cargo.parent.parent), "RUSTUP_HOME": str(root / "rustup"),
                                   "RUNNER_TEMP": str(root / "run"), "GITHUB_ENV": str(root / "github-env")}
                    result = subprocess.run(["/bin/bash", "-c", prefix +
                        '\nprintf "%s\\n" "$ELASTOS_NODE_BIN" "$ELASTOS_BROWSER_VM_TURNSERVER_BIN"\n'],
                        env=environment, capture_output=True, text=True, timeout=10)
                    if missing:
                        self.assertNotEqual(result.returncode, 0, result.stdout)
                    else:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout.splitlines(), [str(tools / "node"), str(tools / "turnserver")])

    def test_source_home_resolvers_accept_managed_or_explicit_tools_and_refuse_path_only(self):
        source = (WORKFLOW.parents[2] / "scripts/setup-source-home.sh").read_text()
        definitions = "\n".join(re.search(rf"(?ms)^{name}\(\) \{{.*?^\}}$", source)[0]
                                for name in ("find_node", "find_turnserver", "find_vz_turn_program"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            tools, managed = root / "tools", root / "data/bin"
            tools.mkdir()
            managed.mkdir(parents=True)
            for parent in (tools, managed):
                for name in ("node", "turnserver"):
                    (parent / name).write_text("#!/bin/bash\nexit 0\n")
                    (parent / name).chmod(0o700)
            environment = {**os.environ, "PATH": str(tools), "DATA_DIR": str(managed.parent)}
            for name in ("ELASTOS_NODE_BIN", "ELASTOS_BROWSER_VM_TURNSERVER_BIN",
                         "ELASTOS_TURNSERVER_BIN", "ELASTOS_BROWSER_VM_TURN_PROGRAM"):
                environment.pop(name, None)
            cases = [("managed", {}, managed),
                     ("explicit", {"ELASTOS_NODE_BIN": str(tools / "node"),
                                   "ELASTOS_BROWSER_VM_TURNSERVER_BIN": str(tools / "turnserver")}, tools),
                     ("path only", {"DATA_DIR": str(root / "empty")}, None),
                     ("relative explicit", {"ELASTOS_NODE_BIN": "node",
                                            "ELASTOS_BROWSER_VM_TURNSERVER_BIN": "turnserver"}, None)]
            for label, overrides, expected in cases:
                for resolver, tool in (("find_node", "node"), ("find_turnserver", "turnserver"),
                                       ("find_vz_turn_program", "turnserver")):
                    with self.subTest(case=label, resolver=resolver):
                        result = subprocess.run(["/bin/bash", "-c", definitions + "\n" + resolver],
                            env={**environment, **overrides}, capture_output=True, text=True, timeout=10)
                        if expected is None:
                            self.assertNotEqual(result.returncode, 0, result.stdout)
                        else:
                            self.assertEqual(result.returncode, 0, result.stderr)
                            self.assertEqual(result.stdout.strip(), str(expected / tool))

    def test_every_ci_action_has_a_full_commit_pin(self):
        for action, pin in re.findall(r'uses: ([\w/-]+)@([^\s]+)', SOURCE):
            self.assertRegex(pin, r'^[0-9a-f]{40}$', action)

    def test_merge_groups_run_every_proof_on_the_queued_commit(self):
        triggers = SOURCE.split("\npermissions:", 1)[0]
        self.assertIn("\n  merge_group:\n    types: [checks_requested]\n", triggers)
        self.assertIn("\n  pull_request:\n", triggers)
        self.assertIn("branches: [main, develop]", triggers)
        context = {"github.event_name": "merge_group", "github.ref": "refs/heads/gh-readonly-queue/develop/pr-1"}
        for job in JOBS:
            if job == "cancel-on-failure":
                continue
            guard = re.search(r"(?m)^    if: (.*)$", JOBS[job])
            if job == "release-upstream-assets":
                self.assertTrue(upstream_assets_run(SOURCE, context, ""), job)
            elif guard:
                self.assertTrue(evaluate(guard[1], context), job)
            checkout, = [step for step in steps(job) if "uses: actions/checkout@" in step]
            self.assertEqual(field(checkout, "ref"),
                             "${{ github.event_name == 'workflow_dispatch' && inputs.ref || github.sha }}")
        for event, ref, ref_type, override, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override}
            context["env.CI_SAVE_CACHE"] = str(evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)).lower()
            expected_job = event in ("merge_group", "workflow_dispatch", "pull_request") or \
                (event == "push" and ref in SAVING_REFS)
            self.assertEqual(job_runs("custody-harness-smoke", context), expected_job)
            for paths in ("true", "false"):
                with self.subTest(event=event, ref=ref, paths=paths):
                    expected = expected_job and (event != "pull_request" or paths == "true")
                    self.assertEqual(custody_should_run(SOURCE, context, paths), expected)
                    # Release upstream builds: changed inputs on a PR, every merge group and manual run.
                    filtered = paths if event == "pull_request" else ""
                    self.assertEqual(upstream_assets_run(SOURCE, context, filtered),
                                     event in ("merge_group", "workflow_dispatch") or filtered == "true")

    def test_sccache_is_scoped_to_test_jobs(self):
        expected = UNIT_CACHE_JOBS | SOURCE_HOME_CACHE_JOBS
        self.assertEqual({job for job in JOBS if any(f"uses: {LOCAL_CACHE_ACTION}" in step
                                                   for step in steps(job))}, expected)
        for job in UNIT_CACHE_JOBS:
            setup, = [step for step in steps(job) if f"uses: {LOCAL_CACHE_ACTION}" in step]
            self.assertEqual(field(setup, "if"), "env.CI_USE_CACHE == 'true'")
            self.assertLess(JOBS[job].index(setup), JOBS[job].index("run: cargo")
                            if job != "test-capsules" else JOBS[job].index("name: lint and test"))
            self.assertIn("run: sccache --show-stats", JOBS[job])
        uncached = SOURCE.split("\njobs:\n", 1)[0] + "\n".join(
            JOBS[job] for job in JOBS if job not in expected)
        self.assertNotRegex(uncached, r"(?i)sccache|RUSTC_WRAPPER|rust-compile-cache")

    def test_compile_free_source_gate_keeps_one_nested_installer_suite(self):
        source_gate = JOBS["source-gate"]
        self.assertNotRegex(source_gate, r"(?i)sccache|RUSTC_WRAPPER|rust-compile-cache")
        self.assertIn("run: ./scripts/test-source-gate.sh release", source_gate)
        self.assertNotIn("python3 scripts/install-bootstrap-test.py", SOURCE_GATE)
        self.assertEqual(SOURCE_GATE.count("python3 scripts/release-platform-input-test.py"), 1)
        nested = (WORKFLOW.parents[2] / "scripts/release-platform-input-test.py").read_text()
        self.assertIn('"install-bootstrap-test.py"', nested)
        self.assertIn('[sys.executable, str(script), "--bash", "/bin/bash"]', nested)

    def test_ci_actions_and_sccache_release_are_immutable(self):
        action = WORKFLOW.parents[1] / "actions/rust-compile-cache/action.yml"
        for path in (WORKFLOW, WORKFLOW.parent / "release-package.yml", action):
            for uses in re.findall(r"(?m)^\s*(?:- )?uses: (\S+)", path.read_text()):
                if uses == LOCAL_CACHE_ACTION:
                    continue
                self.assertRegex(uses, r"^[\w./-]+@[0-9a-f]{40}$", str(path))
        setup_source = action.read_text()
        self.assertIn("sccache-v0.18.0-x86_64-unknown-linux-musl", setup_source)
        self.assertIn("45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89", setup_source)
        self.assertLess(setup_source.index('"${checksum[@]}" -c'), setup_source.index("tar -xzf"))
        self.assertLess(setup_source.index("tar -xzf"), setup_source.index('echo "RUSTC_WRAPPER=sccache"'))

    def test_release_builds_are_hermetic_with_fresh_outputs(self):
        source = (WORKFLOW.parent / "release-package.yml").read_text()
        validate_release_builds_are_hermetic(source)
        for job in ("mac", "linux"):
            job_steps = steps(job, jobs(source))
            parity, = [step for step in job_steps if step.startswith("name: check native versions, source and support parity\n")]
            for token in ('cmp "$inputs/N1/support-input.json" "$inputs/N/platform-input.json"',
                          '--version', '$SOURCE_COMMIT $SOURCE_TREE', 'elastos $version'):
                self.assertIn(token, parity)
            build, = [step for step in job_steps if step.startswith("name: build N, then N+1 with N's support, each in fresh Cargo directories\n")]
            script = textwrap.dedent(build.split("        run: |\n", 1)[1])
            for existing in (None, "target", "build", "target-link", "build-link"):
                with self.subTest(job=job, existing=existing), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    (root / "scripts").mkdir()
                    (root / "shims").mkdir()
                    release = root / "release"
                    release.mkdir()
                    producer = root / "scripts/prepare-release-platform.sh"
                    producer.write_text(f"#!{sys.executable}\n" + textwrap.dedent("""\
                        import json, os, pathlib, sys
                        target = pathlib.Path(os.environ['CARGO_TARGET_DIR'])
                        build = pathlib.Path(os.environ['CARGO_BUILD_BUILD_DIR'])
                        target.mkdir()
                        build.mkdir()
                        output = pathlib.Path(sys.argv[sys.argv.index('--output') + 1])
                        output.mkdir(parents=True)
                        with open(os.environ['BUILD_CALLS'], 'a') as log:
                            log.write(json.dumps({'target': str(target), 'build': str(build),
                                                  'args': sys.argv[1:]}) + '\\n')
                        """))
                    producer.chmod(0o700)
                    verifier = root / "shims/python3"
                    verifier.write_text('#!/bin/bash\n[[ "$1 $2 $3 $4" == "-I -S scripts/release-platform-input.py verify" && -d "$5" ]]\n')
                    verifier.chmod(0o700)
                    target, build_dir = (root / "release-cargo-target", root / "release-cargo-build")
                    if existing:
                        output = target if existing.startswith("target") else build_dir
                        if existing.endswith("-link"):
                            output.symlink_to(root / "missing")
                        else:
                            output.mkdir()
                    calls = root / "calls.jsonl"
                    env = {**os.environ, "PATH": str(root / "shims") + os.pathsep + os.environ["PATH"],
                           "RUNNER_TEMP": str(root), "RELEASE_ROOT": str(release),
                           "CARGO_HOME": str(root / "cargo-home"), "RUSTUP_HOME": str(root / "rustup-home"),
                           "INSTALL_VERSION": "1.2.3", "UPDATE_VERSION": "1.2.4", "BUILD_CALLS": str(calls),
                           "BROWSER_IMAGE_CID": "bafkreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                           "BROWSER_IMAGE_SHA256": "ab" * 32, "BROWSER_IMAGE_SIZE": "1024",
                           "RELEASE_PLATFORM": "aarch64-darwin" if job == "mac" else "aarch64-linux"}
                    result = subprocess.run(["bash", "-c", script], cwd=root, env=env,
                                            capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode == 0, existing is None, result.stderr)
                    if existing:
                        self.assertFalse(calls.exists())
                        self.assertTrue(output.exists() or output.is_symlink())
                    else:
                        built = [json.loads(line) for line in calls.read_text().splitlines()]
                        self.assertEqual(len(built), 2)
                        # Browser image input is Mac-only until #90.
                        image = (['--browser-vm-image-cid', env['BROWSER_IMAGE_CID'],
                                  '--browser-vm-image-sha256', env['BROWSER_IMAGE_SHA256'],
                                  '--browser-vm-image-size', env['BROWSER_IMAGE_SIZE']] if job == "mac" else [])
                        self.assertEqual({item['target'] for item in built}, {str(target)})
                        self.assertEqual({item['build'] for item in built}, {str(build_dir)})
                        self.assertEqual(built[0]['args'], ['--version', '1.2.3', '--output', str(release / 'inputs/N'), *image])
                        self.assertEqual(built[1]['args'], ['--version', '1.2.4', '--output', str(release / 'inputs/N1'),
                                                          '--reuse-support', str(release / 'inputs/N')])
                        self.assertFalse(target.exists() or target.is_symlink())
                        self.assertFalse(build_dir.exists() or build_dir.is_symlink())

    def test_release_policy_rejects_compiler_cache_profile_changes_and_target_reuse(self):
        source = (WORKFLOW.parent / "release-package.yml").read_text()
        toolchain = "      - uses: dtolnay/rust-toolchain@"
        mutations = (
            source.replace(toolchain, "      - name: configure Rust compile cache\n"
                           "        uses: ./source/.github/actions/rust-compile-cache\n" + toolchain, 1),
            source.replace("RUSTC_WRAPPER: ''", "RUSTC_WRAPPER: sccache", 1),
            source.replace("  RUSTC_WRAPPER: ''\n", "", 1),
            source.replace('echo "RELEASE_ROOT=$root" >> "$GITHUB_ENV"',
                           'echo "RELEASE_ROOT=$root" >> "$GITHUB_ENV"\n          echo "RUSTC_WRAPPER=sccache" >> "$GITHUB_ENV"', 1),
            source.replace('[[ ! -e "$CARGO_TARGET_DIR" && ! -L "$CARGO_TARGET_DIR" && ! -e "$CARGO_BUILD_BUILD_DIR" && ! -L "$CARGO_BUILD_BUILD_DIR" ]]', "true", 1),
            source.replace('CARGO_TARGET_DIR="$RUNNER_TEMP/release-cargo-target"', 'CARGO_TARGET_DIR="$RELEASE_ROOT/cargo-$name"', 1),
            source.replace('CARGO_BUILD_BUILD_DIR="$RUNNER_TEMP/release-cargo-build"', 'CARGO_BUILD_BUILD_DIR="$RELEASE_ROOT/cargo-build-$name"', 1),
            source.replace('--reuse-support "$inputs/N"', '--reuse-support "$inputs/other"', 1),
            source + '\nCARGO_PROFILE_RELEASE_LTO=false\n',
            source.replace("actions/cache/restore@", "actions/cache@", 1),
        )
        for index, changed in enumerate(mutations):
            self.assertNotEqual(changed, source)
            with self.subTest(mutation=index), self.assertRaises(AssertionError):
                validate_release_builds_are_hermetic(changed)

    def test_release_engine_restore_matches_ci_recipe_and_always_verifies(self):
        release_jobs = jobs((WORKFLOW.parent / "release-package.yml").read_text())
        engine = steps("engine-llama-arm64", release_jobs)
        recipe, = [step for step in engine if "id: engine-recipe\n" in step]
        ci_recipe, = [step for step in steps("engine-llama-arm64") if "id: engine-recipe\n" in step]
        self.assertEqual(recipe.split("        run: |\n", 1)[1],
                         ci_recipe.split("        run: |\n", 1)[1])
        for key in ("BUILD_CONTAINER_IMAGE", "ENGINE_BUILD_TOOLS"):
            self.assertEqual(field(release_jobs["engine-llama-arm64"], key),
                             field(JOBS["engine-llama-arm64"], key))
        restore, = [step for step in engine if "actions/cache/restore@" in step]
        ci_restore, = [step for step in steps("engine-llama-arm64") if "actions/cache/restore@" in step]
        for key in ("uses", "key", "path"):
            self.assertEqual(field(restore, key), field(ci_restore, key))
        self.assertNotIn("restore-keys:", restore)
        build, = [step for step in engine if "id: engine-build\n" in step]
        for hit in ("true", "false"):
            self.assertEqual(evaluate(field(build, "if"), {"steps.engine-cache.outputs.cache-hit": hit}), hit != "true")
        verify, = [step for step in engine if step.startswith("name: verify accepted ARM64 engine archive\n")]
        self.assertNotIn("if:", verify)
        self.assertIn('--verify-archive "$RUNNER_TEMP/llama-arm64-bundle"', verify)
        upload, = [step for step in engine if "actions/upload-artifact@" in step]
        self.assertLess(engine.index(restore), engine.index(build))
        self.assertLess(engine.index(build), engine.index(verify))
        self.assertLess(engine.index(verify), engine.index(upload))

    def test_release_upstream_assets_build_every_source_platform_as_the_release_does(self):
        recipes = json.loads((REPO / "scripts/release-upstream-recipes.json").read_text())["recipes"]
        rows = matrix_rows(JOBS["release-upstream-assets"])
        self.assertEqual({row["platform"] for row in rows}, {recipe["platform"] for recipe in recipes if "build" in recipe})
        release_jobs = jobs((WORKFLOW.parent / "release-package.yml").read_text())
        release_names = {"aarch64-linux": "linux-arm64", "x86_64-linux": "linux-amd64"}
        release = {"darwin-arm64": ("mac", {"os": field(release_jobs["mac"], "runs-on")}),
                   **{release_names[row["platform"]]: ("linux", row) for row in matrix_rows(release_jobs["linux"])}}

        def toolchain(job, workflow_jobs, row):
            step, = [step for step in steps(job, workflow_jobs) if "uses: dtolnay/rust-toolchain@" in step]
            version = re.search(r"(?m)^\s+toolchain: (\S+)$", step) or re.search(r" # (\S+)$", field(step, "uses"))
            return version[1], field(step, "targets").replace("${{ matrix.target }}", row.get("target", ""))

        def host_packages(job, workflow_jobs, row):
            text = "\n".join(steps(job, workflow_jobs)).replace('"${{ matrix.host }}"', row.get("host", ""))
            self.assertNotRegex(text, r"apt-get|brew install", job)
            return re.findall(r"(?m)^\s+(?:run: )?(bash scripts/release-host-prerequisites\.sh \w+)$", text)

        for row in rows:
            job, release_row = release[row["platform"]]
            with self.subTest(platform=row["platform"]):
                self.assertEqual(row["os"], release_row["os"])
                self.assertEqual(toolchain("release-upstream-assets", JOBS, row), toolchain(job, release_jobs, release_row))
                packages = host_packages("release-upstream-assets", JOBS, row)
                self.assertEqual(len(packages), 1)
                self.assertEqual(packages, host_packages(job, release_jobs, release_row))

    def test_sccache_jobs_keep_registry_caches_without_target_artifacts(self):
        for job in ("source-gate", "lint", "test-elastos", "test-capsules"):
            for step in steps(job):
                if "Swatinem/rust-cache@" in step:
                    self.assertEqual(field(step, "cache-targets"), "false", job)
                    self.assertEqual(field(step, "cache-bin"), "false", job)
                self.assertNotRegex(step, r"uses: actions/cache(?:/restore|/save)?@", job)
        for job in UNIT_CACHE_JOBS | SOURCE_HOME_CACHE_JOBS:
            job_steps = steps(job)
            setup, = [index for index, step in enumerate(job_steps) if f"uses: {LOCAL_CACHE_ACTION}" in step]
            registry, = [index for index, step in enumerate(job_steps) if "Swatinem/rust-cache@" in step]
            # The trusted registry cache restores and saves the action's local objects.
            self.assertLess(setup, registry, job)
            self.assertEqual(field(job_steps[registry], "cache-directories"), "${{ env.SCCACHE_DIR }}", job)

    def test_sccache_failures_leave_normal_builds_enabled(self):
        source = CACHE_ACTION.read_text()
        for download, server, expected in ((22, 0, False), (0, 1, False), (0, 0, True)):
            with self.subTest(download=download, server=server):
                status, exported, daemon = run_compile_cache_fixture(source, download, server)
                self.assertEqual(status == 0, download == 0)
                self.assertEqual(daemon is not None, download == 0)
                self.assertEqual(exported.get("RUSTC_WRAPPER"), "sccache" if expected else None)
        self.assertEqual(field(source, "continue-on-error"), "true")

    def test_sccache_daemon_holds_no_actions_credential(self):
        action = CACHE_ACTION.read_text()
        validate_cache_credential_scope(SOURCE, action)
        _, exported, daemon = run_compile_cache_fixture(action)
        self.assertEqual(exported["RUSTC_WRAPPER"], "sccache")
        self.assertEqual(daemon["SCCACHE_DIR"], exported["SCCACHE_DIR"])
        self.assertEqual(daemon["SCCACHE_IDLE_TIMEOUT"], "0")
        self.assertFalse([name for name in {**daemon, **exported} if name.startswith(("ACTIONS_", "SCCACHE_GHA"))])
        node = ("    - name: start Rust compile cache\n"
                "      uses: actions/github-script@f28e40c7f34bde8b3046d885e986cb6290c5673b # v7\n"
                "      with:\n        script: |\n"
                "          spawnSync('sccache', ['--start-server'], {env: process.env});\n")
        check = re.search(r"(?ms)^      - name: Rust compile cache holds no Actions credential\n.*?(?=^      - |\Z)",
                          JOBS["lint"])[0]
        mutations = {
            "Node step starts the daemon": (SOURCE, action + node),
            "action exports the runtime": (SOURCE, action.replace(
                'echo "CARGO_INCREMENTAL=0"', 'echo "CARGO_INCREMENTAL=0"\n            echo "ACTIONS_RUNTIME_TOKEN=$t"', 1)),
            "action enables a remote cache": (SOURCE, action.replace(
                'SCCACHE_IDLE_TIMEOUT=0', 'SCCACHE_IDLE_TIMEOUT=0 SCCACHE_GHA_ENABLED=on', 1)),
            "workflow exports the runtime": (SOURCE.replace(
                "      - name: Rust compile cache statistics\n",
                "      - uses: actions/github-script@f28e40c7f34bde8b3046d885e986cb6290c5673b # v7\n"
                "        with:\n          script: core.exportVariable('ACTIONS_RUNTIME_TOKEN', process.env.ACTIONS_RUNTIME_TOKEN)\n"
                "      - name: Rust compile cache statistics\n", 1), action),
            "workflow writes the runtime to GITHUB_ENV": (SOURCE.replace(
                'echo "CARGO_PROFILE_RELEASE_LTO=false" >> "$GITHUB_ENV"',
                'echo "CARGO_PROFILE_RELEASE_LTO=false" >> "$GITHUB_ENV"\n'
                '          echo "ACTIONS_RUNTIME_TOKEN=$t" >> "$GITHUB_ENV"', 1), action),
            "workflow maps the runtime into a step": (SOURCE.replace(
                "        run: cargo clippy", "        env:\n          ACTIONS_RUNTIME_TOKEN: x\n        run: cargo clippy", 1), action),
            "workflow restores a remote level": (SOURCE.replace(
                'echo "CARGO_TARGET_DIR=$RUNNER_TEMP/source-home-macos-target" >> "$GITHUB_ENV"',
                'echo "SCCACHE_MULTILEVEL_CHAIN=disk,gha" >> "$GITHUB_ENV"', 1), action),
            "a job loses its check": (SOURCE.replace(check, "", 1), action),
            "a check precedes later builds": (SOURCE.replace(check, "", 1).replace(
                "      - name: cargo clippy\n", check + "      - name: cargo clippy\n", 1), action),
            "a check never runs": (SOURCE.replace(
                check, check.replace("if: always() && env.RUSTC_WRAPPER == 'sccache'", "if: false", 1), 1), action),
            "a check is weakened": (SOURCE.replace(
                check, check.replace('test -z "${ACTIONS_RUNTIME_TOKEN:-}"', "true", 1), 1), action),
        }
        for name, (source, candidate) in mutations.items():
            self.assertNotEqual((source, candidate), (SOURCE, action), name)
            with self.subTest(mutation=name), self.assertRaises(AssertionError):
                validate_cache_credential_scope(source, candidate)

    def test_credential_check_fails_on_a_daemon_or_step_holding_the_runtime(self):
        script = validate_cache_credential_scope(SOURCE, CACHE_ACTION.read_text())
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            pgrep = root / "pgrep"
            pgrep.write_text('#!/bin/bash\n[ "$*" = "-x sccache" ] || exit 99\n[ -n "$DAEMON" ] || exit 1\necho "$DAEMON"\n')
            pgrep.chmod(0o700)
            base = {name: value for name, value in os.environ.items()
                    if not name.startswith(("ACTIONS_", "SCCACHE_"))}
            base.update(PATH=f"{root}{os.pathsep}{os.environ['PATH']}",
                        RUNNER_OS="Linux" if Path("/proc/self/environ").exists() else "macOS")

            def check(daemon_env, step_env=()):
                # macOS hides the environment of platform binaries such as sleep.
                daemon = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"],
                                          env={**base, **daemon_env}) if daemon_env is not None else None
                try:
                    return subprocess.run(["bash", "-e", "-c", script], capture_output=True, text=True, timeout=10,
                                          env={**base, **dict(step_env), "DAEMON": str(daemon.pid) if daemon else ""}).returncode
                finally:
                    if daemon:
                        daemon.kill()
                        daemon.wait()

            daemon = {"SCCACHE_DIR": str(root / "objects")}
            self.assertEqual(check(daemon), 0)
            self.assertNotEqual(check(daemon, {"ACTIONS_RUNTIME_TOKEN": "fixture"}), 0)
            self.assertNotEqual(check({**daemon, "ACTIONS_RUNTIME_TOKEN": "fixture"}), 0)
            # A check that reads no daemon environment proves nothing.
            self.assertNotEqual(check(None), 0)
            self.assertNotEqual(check({}), 0)

    def test_sccache_smoke_clients_discard_inherited_runtime(self):
        smoke = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-rust-cache-smoke.py"))
        inherited = {"ACTIONS_RUNTIME_TOKEN": "real-job-token-fixture",
                     "ACTIONS_RESULTS_URL": "https://real-job.invalid/",
                     "ACTIONS_CACHE_SERVICE_V2": "True", "SCCACHE_GHA_RW_MODE": "READ_WRITE",
                     "CARGO_TARGET_DIR": "/real-job-target", "OPENDAL_TEST": "ghac"}
        with mock.patch.dict(os.environ, inherited):
            clients = smoke["client_environment"]()
        self.assertFalse(set(inherited) & clients.keys())

    def test_journey_profiles_and_fresh_targets_are_job_scoped(self):
        release = (WORKFLOW.parent / "release-package.yml").read_text()
        validate_journey_builds(SOURCE, release)

    def test_journey_build_policy_rejects_profile_leaks_and_split_targets(self):
        release = (WORKFLOW.parent / "release-package.yml").read_text()
        mutations = (
            SOURCE.replace('CARGO_PROFILE_RELEASE_LTO=false', 'CARGO_PROFILE_RELEASE_LTO=true', 1),
            SOURCE.replace('CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1', 1),
            SOURCE.replace('source-home-linux-target', 'separate-target', 1),
            SOURCE.replace('name: set job build directories\n',
                           'name: set job build directories\n        if: false\n', 1),
            SOURCE.replace('  source-home-linux:\n',
                           '  source-home-linux:\n    env:\n      CARGO_TARGET_DIR: ${{ runner.temp }}/source-home-linux-target\n', 1),
            SOURCE.replace("if: github.event_name == 'pull_request' || github.event_name == 'merge_group' || (github.event_name == 'push' && github.ref == 'refs/heads/develop')",
                           "if: github.event_name == 'push'", 1),
            SOURCE.replace('name: require fresh Linux build artifacts\n',
                           'name: require fresh Linux build artifacts\n        if: false\n', 1),
            SOURCE.replace('name: require fresh Mac build intermediates\n',
                           'name: require fresh Mac build intermediates\n        if: false\n', 1),
            SOURCE.replace('test ! -L "$directory"', 'true', 1),
            SOURCE.replace('journey artifacts start fresh.\n          cache-targets: false',
                           'journey artifacts start fresh.\n          cache-targets: true', 1),
            SOURCE.replace('scripts/setup-source-home.sh 2>&1', 'CARGO_TARGET_DIR=other scripts/setup-source-home.sh 2>&1', 1),
            SOURCE.replace('  lint:\n', '  lint:\n    env:\n      CARGO_PROFILE_RELEASE_LTO: "false"\n', 1),
        )
        for index, changed in enumerate(mutations):
            self.assertNotEqual(changed, SOURCE)
            with self.subTest(mutation=index), self.assertRaises(AssertionError):
                validate_journey_builds(changed, release)
        with self.assertRaises(AssertionError):
            validate_journey_builds(SOURCE, release + '\nCARGO_PROFILE_RELEASE_LTO: "false"\n')

    def test_fresh_job_directories_refuse_restored_outputs_and_links(self):
        for job in SOURCE_HOME_CACHE_JOBS:
            paths, = [step for step in steps(job) if step.startswith("name: set job build directories\n")]
            exports = textwrap.dedent(paths.split("        run: |\n", 1)[1])
            fresh, = [step for step in steps(job) if 'test ! -e "$directory"' in step]
            script = textwrap.dedent(fresh.split("        run: |\n", 1)[1])
            for existing in (None, "target", "build", "link"):
                with self.subTest(job=job, existing=existing), tempfile.TemporaryDirectory() as directory:
                    root = Path(directory)
                    env = {**os.environ, "RUNNER_TEMP": str(root), "GITHUB_ENV": str(root / "env")}
                    subprocess.run(["bash", "-e", "-c", exports], env=env, check=True)
                    env.update(dict(line.split("=", 1) for line in (root / "env").read_text().splitlines()))
                    target, build = Path(env["CARGO_TARGET_DIR"]), Path(env["CARGO_BUILD_BUILD_DIR"])
                    directory_job = "source-home-linux" if job == "source-home-linux-arm64" else job
                    self.assertEqual(target, root / (directory_job + "-target"))
                    self.assertEqual(build, root / (directory_job + "-build"))
                    self.assertFalse(target.exists())
                    self.assertFalse(build.exists())
                    if existing == "link":
                        target.symlink_to(root / "missing")
                    elif existing:
                        (target if existing == "target" else build).mkdir()
                    result = subprocess.run(["bash", "-c", script], capture_output=True, text=True,
                                            env=env)
                    self.assertEqual(result.returncode == 0, existing is None, result.stderr)

    def test_packages_select_shared_outputs_and_preserve_binary_hashes(self):
        for shared in (False, True):
            with self.subTest(shared=shared), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                script = root / "scripts/package-release-binaries.sh"
                script.parent.mkdir()
                script.write_bytes((WORKFLOW.parents[2] / "scripts/package-release-binaries.sh").read_bytes())
                shims = root / "shims"
                shims.mkdir()
                cargo = shims / "cargo"
                cargo.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''\
                    import json, pathlib, sys
                    assert sys.argv[1:6] == ['metadata', '--locked', '--offline', '--no-deps', '--format-version']
                    manifest = pathlib.Path(sys.argv[sys.argv.index('--manifest-path') + 1])
                    name = 'elastos' if manifest.parent.name == 'elastos' else 'custody-provider'
                    print(json.dumps({'workspace_members': ['member'], 'packages': [
                        {'id': 'member', 'targets': [{'name': name, 'kind': ['bin']},
                                                    {'name': 'unused.rlib', 'kind': ['lib']}]},
                        {'id': 'dependency', 'targets': [{'name': 'dependency-tool', 'kind': ['bin']}]}]}))
                    '''))
                cargo.chmod(0o755)
                capsule = root / 'capsules/custody-provider'
                capsule.mkdir(parents=True)
                (capsule / 'Cargo.lock').touch()
                target = root / "shared"
                env = dict(os.environ)
                env["COPYFILE_DISABLE"] = "1"
                env["PATH"] = str(shims) + os.pathsep + env["PATH"]
                env.pop("CARGO_TARGET_DIR", None)
                if shared:
                    env["CARGO_TARGET_DIR"] = str(target)
                outputs = target / "release" if shared else root / "elastos/target/release"
                outputs.mkdir(parents=True)
                for name in ("elastos", "unused.rlib", "dependency-tool", "browser-local-exit",
                             "browser-engine-supervisor", "browser-native-proxy-engine", "browser-stream-bridge"):
                    path = outputs / name
                    path.write_bytes(name.encode())
                    path.chmod(0o755)
                capsule_outputs = outputs if shared else capsule / "target/release"
                capsule_outputs.mkdir(parents=True, exist_ok=True)
                provider = capsule_outputs / "custody-provider"
                provider.write_bytes(b"custody-provider")
                provider.chmod(0o755)
                if shared:
                    stale = root / "elastos/target/release/stale"
                    stale.parent.mkdir(parents=True)
                    stale.write_bytes(b"restored artifact")
                    stale.chmod(0o755)
                subprocess.run(["bash", str(script)], cwd=root, env=env, capture_output=True, check=True)
                package, = root.glob("*.tar.gz")
                checksum = Path(str(package) + ".sha256").read_text().split()[0]
                self.assertEqual(checksum, hashlib.sha256(package.read_bytes()).hexdigest())
                with tarfile.open(package) as archive:
                    binaries = {Path(item.name).name: hashlib.sha256(archive.extractfile(item).read()).hexdigest()
                                for item in archive if item.isfile()}
                self.assertEqual(binaries, {name: hashlib.sha256(name.encode()).hexdigest()
                                            for name in ("elastos", "custody-provider")})

    def test_carrier_binary_paths_use_the_shared_target_or_workspace_default(self):
        source = (WORKFLOW.parents[2] / "scripts/local-carrier-setup-smoke.sh").read_text()
        function = "cargo_release_binary() {" + source.split("cargo_release_binary() {", 1)[1].split("\n}\n", 1)[0] + "\n}"
        self.assertNotIn("/target/release/", source)
        for target in ("", "/shared job/target"):
            env = {**os.environ, "CARGO_TARGET_DIR": target, "REPO_ROOT": "/checkout"}
            for workspace, name in (("elastos", "elastos"), ("elastos", "localhost-provider"),
                                    ("capsules/custody-provider", "custody-provider")):
                result = subprocess.run(["bash", "-c", function + '\ncargo_release_binary "$1" "$2"', "paths", workspace, name],
                                        env=env, capture_output=True, text=True, check=True)
                self.assertEqual(result.stdout.strip(), f"{target or '/checkout/' + workspace + '/target'}/release/{name}")

        prefix = source.split('\nif [[ "$(uname -s)" != "Linux" ]]', 1)[0]
        runtime_build, = re.findall(r'(?m)^\(cd "\$\{ELASTOS_ROOT\}" && cargo build[^\n]*\)$', source)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "elastos").mkdir()
            script = root / "scripts/carrier-runtime-fixture.sh"
            script.parent.mkdir()
            script.write_text(prefix + '\n' + runtime_build + '\nprintf "%s\\n" "$ELASTOS_BIN"\n')
            shims = root / "shims"
            shims.mkdir()
            cargo = shims / "cargo"
            cargo.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''\
                import json, os, pathlib, sys
                pathlib.Path(os.environ["CARGO_CALLS"]).write_text(json.dumps({
                    "args": sys.argv[1:], "target": os.environ.get("CARGO_TARGET_DIR")}))
                sys.exit(int(os.environ.get("CARGO_STATUS", "0")))
                '''))
            cargo.chmod(0o755)
            for target in (None, "/shared job/target", "relative target"):
                with self.subTest(target=target):
                    env = {**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
                           "CARGO_CALLS": str(root / "calls")}
                    env.pop("CARGO_TARGET_DIR", None)
                    env.pop("CARGO_STATUS", None)
                    if target is not None:
                        env["CARGO_TARGET_DIR"] = target
                    normalized = str(root / target) if target and not target.startswith("/") else target
                    result = subprocess.run(["bash", str(script)], env=env, capture_output=True, text=True)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(result.stdout.strip(), f"{normalized or str(root / 'elastos/target')}/release/elastos")
                    calls = json.loads((root / "calls").read_text())
                    self.assertEqual(calls, {"args": ["build", "--locked", "--release", "-p", "elastos-server"],
                                             "target": normalized})
                    # A failed source freshness check must stop before using the binary.
                    refused = subprocess.run(["bash", str(script)], env={**env, "CARGO_STATUS": "19"},
                                             capture_output=True, text=True)
                    self.assertEqual(refused.returncode, 19)
                    self.assertEqual(refused.stdout, "")

    def test_component_build_and_componentizer_share_the_job_target(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            capsule = root / "capsule"
            capsule.mkdir()
            (capsule / "Cargo.toml").write_text('[package]\nname="fixture"\nversion="0.1.0"\n')
            wit = WORKFLOW.parents[2] / "elastos/wit/elastos-bus-v1.wit"
            (capsule / "capsule.json").write_text(json.dumps({"runtime_abi": "elastos.component/v1",
                "bus_contract": "elastos:bus@v1", "execution": "component", "entrypoint": "fixture.component.wasm",
                "wit_world_sha256": hashlib.sha256(wit.read_bytes()).hexdigest()}))
            cargo = root / "fake-cargo"
            cargo.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''
                import json, os, pathlib, sys
                args = sys.argv[1:]
                target = pathlib.Path(os.environ['CARGO_TARGET_DIR'])
                with open(os.environ['BUILD_CALLS'], 'a') as log:
                    log.write(json.dumps({'command': args[0], 'target': str(target)}) + '\\n')
                if args[0] == 'metadata':
                    print(json.dumps({'packages': [{'manifest_path': args[args.index('--manifest-path') + 1],
                          'targets': [{'crate_types': ['cdylib'], 'name': 'fixture'}]}]}))
                elif args[0] == 'build':
                    output = target / 'wasm32-unknown-unknown/release/fixture.wasm'
                    output.parent.mkdir(parents=True)
                    output.write_bytes(b'fixture wasm')
                else:
                    source, output = map(pathlib.Path, args[args.index('--') + 1:])
                    output.write_bytes(source.read_bytes())
            '''))
            cargo.chmod(0o755)
            rustc = root / "fake-rustc"
            rustc.write_text("#!/bin/sh\nprintf '/fixture/rust\\n'\n")
            rustc.chmod(0o755)
            target = root / "shared target"
            env = {**os.environ, "CARGO_TARGET_DIR": str(target), "CARGO_BIN": str(cargo),
                   "RUSTC_BIN": str(rustc), "BUILD_CALLS": str(root / "calls")}
            subprocess.run(["bash", str(WORKFLOW.parents[2] / "scripts/build-component-capsule.sh"), str(capsule)],
                           env=env, capture_output=True, text=True, check=True)
            calls = [json.loads(line) for line in (root / "calls").read_text().splitlines()]
            self.assertEqual([call['command'] for call in calls], ['metadata', 'build', 'run'])
            self.assertEqual({call['target'] for call in calls}, {str(target)})
            self.assertEqual((capsule / "fixture.component.wasm").read_bytes(), b'fixture wasm')

    def test_event_ref_matrix_controls_every_cache_action(self):
        validate_cache_guards(SOURCE)
        caches = [(job, step) for job in JOBS for step in steps(job)
                  if CACHE_RE.search(step)]
        self.assertEqual(len(caches), 33)
        for event, ref, ref_type, override, cached in CASES:
            with self.subTest(event=event, ref=ref, override=override):
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override}
                use_cache = evaluate(field(SOURCE, "CI_USE_CACHE"), context)
                self.assertEqual(use_cache, cached)
                context["env.CI_USE_CACHE"] = str(use_cache).lower()
                save = evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)
                self.assertEqual(save, event == "push" and ref == CACHE_SAVING_REF)
                context["env.CI_SAVE_CACHE"] = str(save).lower()
                context.update(NO_CACHE_HIT)
                for paths in ("true", "false"):
                    should_run = custody_should_run(SOURCE, context, paths)
                    context["steps.should-run.outputs.run"] = str(should_run).lower()
                    for kubo_hit in ("true", "false"):
                        context["steps.kubo-cache.outputs.cache-hit"] = kubo_hit
                        for job, step in caches:
                            for apt_changed in ("true", "false"):
                                context["steps.apt-prerequisites.outputs.changed"] = apt_changed
                                allowed = job_runs(job, context)
                                expected = allowed and cached and (job != "custody-harness-smoke" or should_run)
                                if "actions/cache/save@" in step:
                                    expected = expected and (event == "push" and ref == "refs/heads/develop"
                                                             if job == "engine-llama-arm64" else save)
                                    if step.startswith("name: save verified Kubo inputs\n"):
                                        expected = expected and kubo_hit != "true"
                                    if step.startswith("name: save Ubuntu prerequisite archives\n"):
                                        expected = expected and apt_changed == "true"
                                self.assertEqual(allowed and evaluate(field(step, "if"), context), expected,
                                                 f"cache guard in {job} (Kubo hit={kubo_hit}, apt changed={apt_changed})")

    def test_custody_smoke_always_runs_on_develop_and_filters_only_prs(self):
        self.assertNotIn("changes", list(JOBS))
        self.assertEqual(field(JOBS["custody-harness-smoke"], "needs"), "source-gate")
        filter_step, = [step for step in steps("custody-harness-smoke") if "id: filter" in step]
        self.assertEqual(field(filter_step, "if"), "github.event_name == 'pull_request'")
        self.assertEqual(field(filter_step, "uses"), "dorny/paths-filter@0e4a8c6effa4802afeda77dc8d303f8176d7dfad # v3")
        decision, = [step for step in steps("custody-harness-smoke") if "id: should-run" in step]
        script = textwrap.dedent(decision.split("        run: |\n", 1)[1])
        for event, ref, _, _, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref}
            eligible = event in ("pull_request", "merge_group", "workflow_dispatch") or (event == "push" and ref in SAVING_REFS)
            self.assertEqual(evaluate(field(JOBS["custody-harness-smoke"], "if"), context), eligible)
            self.assertEqual(evaluate(field(filter_step, "if"), context), event == "pull_request")
            if not eligible:
                continue
            for changed in ("true", "false", ""):
                values = {**context, "steps.filter.outputs.custody": changed,
                          "env.CI_SAVE_CACHE": str(event == "push" and ref == CACHE_SAVING_REF).lower()}
                command = re.sub(r"\$\{\{\s*(.*?)\s*\}\}", lambda match: values[match[1]], script)
                with tempfile.TemporaryDirectory() as root:
                    output = Path(root) / "output"
                    subprocess.run(["bash", "-eo", "pipefail", "-c", command], check=True,
                                   env={**os.environ, "GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(Path(root) / "summary")})
                    expected = event != "pull_request" or changed == "true"
                    self.assertEqual(output.read_text().strip(), "run=" + str(expected).lower())
        # A filter error fails the required job; the decision step has the default success guard.
        self.assertNotIn("continue-on-error", filter_step)
        self.assertNotIn("if:", decision)

    def test_path_filters_cover_every_input_their_jobs_call(self):
        validate_filter_inputs(WORKFLOW, SOURCE, "custody-harness-smoke", custody_filter(SOURCE))
        canary = CANARY.read_text()
        validate_filter_inputs(CANARY, canary, "holder", canary_filter(canary))
        for entry in ("'scripts/setup-source-home.sh'", "'capsules/chain-provider/**'", "'elastos/Cargo.lock'",
                      "'rust-toolchain.toml'", "'.github/workflows/ci.yml'", "'.cargo/config.toml'",
                      "'elastos/.cargo/config.toml'", "'elastos/crates/elastos-auth/**'"):
            with self.subTest(removed=entry):
                trimmed = SOURCE.replace(f"              - {entry}\n", "", 1)
                self.assertNotEqual(trimmed, SOURCE)
                with self.assertRaisesRegex(AssertionError, "custody-harness-smoke filter misses inputs: .*"
                                            + re.escape(entry.strip("'").rstrip("*/"))):
                    validate_filter_inputs(WORKFLOW, trimmed, "custody-harness-smoke", custody_filter(trimmed))
        for entry in ("scripts/update-hop-compare.py", "scripts/seed-kubo-cache.sh", "components.json",
                      "elastos/Cargo.lock", "scripts/release-upstream-input.py", ".cargo/config.toml",
                      "elastos/capsules/localhost-provider/**", "elastos/crates/elastos-guest/**"):
            with self.subTest(removed=entry):
                trimmed = canary.replace(f"      - {entry}\n", "", 1)
                self.assertNotEqual(trimmed, canary)
                with self.assertRaisesRegex(AssertionError, "holder filter misses inputs: .*" + re.escape(entry.rstrip("*/"))):
                    validate_filter_inputs(CANARY, trimmed, "holder", canary_filter(trimmed))
        upstream = upstream_filter(SOURCE)
        validate_filter_inputs(WORKFLOW, SOURCE, "release-upstream-assets", upstream)
        # Following the Python entry point reaches its helper modules, its recipes and the
        # files they package; the step itself reaches the workflow and the shell script.
        for entry in ("scripts/release-upstream-input.py", "scripts/release-upstream-recipes.json",
                      "scripts/browser-upstream-build.py", "scripts/build/licenses/libcap-LICENSE.txt",
                      "scripts/release-host-prerequisites.sh", ".github/workflows/ci.yml"):
            with self.subTest(removed=entry):
                self.assertIn(entry, upstream)
                with self.assertRaisesRegex(AssertionError, "release-upstream-assets filter misses inputs: .*" + re.escape(entry)):
                    validate_filter_inputs(WORKFLOW, SOURCE, "release-upstream-assets", [p for p in upstream if p != entry])
        # A new script call joins the job's inputs; one level of `*` stays in its folder.
        called = canary.replace("          python3 -I -S scripts/holder-idle-ci.py\n",
                                "          python3 -I -S scripts/holder-idle-ci.py\n          scripts/ci-apt-prerequisites.sh\n", 1)
        self.assertNotEqual(called, canary)
        with self.assertRaisesRegex(AssertionError, "scripts/ci-apt-prerequisites.sh"):
            validate_filter_inputs(CANARY, called, "holder", canary_filter(called))
        self.assertTrue(glob_regex("capsules/chain-provider/**").match("capsules/chain-provider/src/main.rs"))
        self.assertFalse(glob_regex("elastos/crates/elastos-protected-content-*/**").match("elastos/crates/x/elastos-protected-content-a/b"))

    def test_only_pr_runs_share_a_cancellable_concurrency_group(self):
        concurrency = SOURCE.split("\nconcurrency:\n", 1)[1].split("\nenv:\n", 1)[0]
        group = field(concurrency, "group")
        for event, ref, _, _, _ in CASES:
            groups = []
            for run_id in ("101", "102", "103"):
                context = {"github.workflow": "CI", "github.event_name": event,
                           "github.ref": ref, "github.run_id": run_id}
                groups.append(re.sub(r"\$\{\{\s*(.*?)\s*\}\}", lambda match: str(evaluate(match[1], context)), group))
                self.assertEqual(evaluate(field(concurrency, "cancel-in-progress"), context), event == "pull_request")
            self.assertEqual(len(set(groups)), 1 if event == "pull_request" else 3)

    def test_jetson_package_exists_before_verification_on_every_event(self):
        validate_jetson_package_lifecycle(SOURCE)
        guard = "name: build and package release binaries\n        if: matrix.os == 'ubuntu-22.04-arm'"
        self.assertIn(guard, JOBS["source-home-linux"])
        regressed = SOURCE.replace(guard, "name: build and package release binaries\n        if: github.event_name != 'pull_request'", 1)
        with self.assertRaisesRegex(AssertionError, "lacks its package on pull_request ubuntu-22.04-arm"):
            validate_jetson_package_lifecycle(regressed)
        omitted = SOURCE.replace("scripts/package-release-binaries.sh", "echo package omitted", 1)
        with self.assertRaisesRegex(AssertionError, "requires the package producer"):
            validate_jetson_package_lifecycle(omitted)
        unlocked = SOURCE.replace("cargo build --locked --manifest-path elastos/Cargo.toml --workspace",
                                  "cargo build --manifest-path elastos/Cargo.toml --workspace", 1)
        with self.assertRaisesRegex(AssertionError, "require --locked"):
            validate_jetson_package_lifecycle(unlocked)

    def test_every_tracked_lockfile_is_audited_on_every_event(self):
        validate_dependency_audit(SOURCE)
        audit = "name: audit every Cargo.lock\n        working-directory: elastos\n"
        for old, new, message in (
                ("cargo-audit:\n    name: cargo-audit\n",
                 "cargo-audit:\n    name: cargo-audit\n    if: github.event_name == 'pull_request'\n", "every event"),
                (audit, audit + "        continue-on-error: true\n", "every event"),
                (audit, "name: audit every Cargo.lock\n        working-directory: .\n", "reviewed exceptions"),
                ('--file "../$lock"', '--ignore RUSTSEC-0000-0000 --file "../$lock"', "reviewed exceptions"),
                ("| sha256sum -c -", "| cat", "pinned digest"),
                (' || failed=1', '', "every tracked Cargo.lock"),
                ('exit "$failed"', 'exit 0', "result ignores Cargo.lock"),
                ('audit --db "$db" --file', 'audit --db "$db" --no-fetch --file', "online yanked")):
            with self.subTest(new=new):
                self.assertIn(old, SOURCE)
                with self.assertRaisesRegex(AssertionError, message):
                    validate_dependency_audit(SOURCE.replace(old, new, 1))

    def test_only_the_arm_compatibility_gate_packages_and_nothing_publishes_binaries(self):
        # Signed releases over Carrier are the only distribution; CI uploads no release packages.
        self.assertNotRegex(SOURCE, r"action-gh-release|elastos-runtime-\*|contents: write")
        for job, runner in (("source-home-linux", "ubuntu-24.04"),
                            ("source-home-linux-arm64", "ubuntu-22.04-arm"),
                            ("source-home-macos", "macos-14")):
            packages = [step for step in steps(job) if step.startswith("name: build and package release binaries\n")]
            for event, ref, ref_type, override, _ in CASES:
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override, "matrix.os": runner}
                with self.subTest(job=job, event=event, ref=ref, override=override):
                    self.assertEqual(any(evaluate(field(step, "if"), context) for step in packages),
                                     job == "source-home-linux-arm64")

    def test_linux_steps_alias_is_exact_and_rejects_other_bindings(self):
        self.assertEqual(steps("source-home-linux-arm64"), steps("source-home-linux"))
        for old, new in (("steps: *source_home_linux_steps", "steps: *unknown_steps"),
                         ("steps: &source_home_linux_steps", "steps: &unknown_steps")):
            with self.subTest(new=new), self.assertRaises(AssertionError):
                steps("source-home-linux-arm64", jobs(SOURCE.replace(old, new, 1)))

    def test_pull_requests_never_save_shared_caches(self):
        for job in JOBS:
            for step in steps(job):
                if "Swatinem/rust-cache@" in step:
                    self.assertEqual(field(step, "save-if"), "${{ env.CI_SAVE_CACHE == 'true' }}",
                                     f"rust-cache in {job} must use the trusted writer guard")
                if "actions/cache@" in step:
                    self.fail(f"{job} uses actions/cache, which also saves from PR runs")

    def test_every_action_is_pinned_to_one_commit_with_its_version(self):
        validate_action_pins(SOURCE, CACHE_ACTION.read_text())

    def test_action_pins_reject_unknown_local_and_mutable_nested_actions(self):
        action = CACHE_ACTION.read_text()
        with self.assertRaisesRegex(AssertionError, "unrecognized local action"):
            validate_action_pins(SOURCE.replace(LOCAL_CACHE_ACTION, "./.github/actions/unknown"), action)
        nested = action + "    - uses: actions/github-script@v7\n"
        with self.assertRaisesRegex(AssertionError, "mutable action"):
            validate_action_pins(SOURCE, nested)

    def test_source_home_cache_action_follows_actual_event_policy(self):
        action = CACHE_ACTION.read_text()
        expected_jobs = SOURCE_HOME_CACHE_JOBS
        self.assertEqual({job for job in JOBS if any(f"uses: {LOCAL_CACHE_ACTION}" in step
                                                   for step in steps(job))},
                         UNIT_CACHE_JOBS | expected_jobs)
        for job in expected_jobs:
            environment = JOBS[job].split("    steps:", 1)[0]
            # The shared action owns every sccache setting.
            self.assertNotIn("SCCACHE_", environment)
            setup, = [step for step in steps(job) if f"uses: {LOCAL_CACHE_ACTION}" in step]
            job_source = "\n".join(steps(job))
            self.assertNotIn("SCCACHE_", job_source.split(setup, 1)[0])
            self.assertLess(job_source.index("uses: dtolnay/rust-toolchain@"), job_source.index(setup))
            self.assertLess(job_source.index(setup), job_source.index("scripts/setup-source-home.sh"))
            statistics, = [step for step in steps(job) if step.startswith("name: Rust compile cache statistics\n")]
            self.assertEqual(field(statistics, "if"), "always() && env.RUSTC_WRAPPER == 'sccache'")
            self.assertIn("sccache --show-stats", statistics)
            self.assertIn("sccache --show-stats --stats-format=json", statistics)
        for event, ref, ref_type, override, cached in CASES:
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override}
            context["env.CI_USE_CACHE"] = str(evaluate(field(SOURCE, "CI_USE_CACHE"), context)).lower()
            for job in expected_jobs:
                setup, = [step for step in steps(job) if f"uses: {LOCAL_CACHE_ACTION}" in step]
                self.assertEqual(evaluate(field(setup, "if"), context), cached, job)
        _, exported, _ = run_compile_cache_fixture(action)
        self.assertEqual(exported["RUSTC_WRAPPER"], "sccache")
        self.assertEqual(exported["CARGO_INCREMENTAL"], "0")

    def test_sccache_platform_pins_and_activation_failures(self):
        action = CACHE_ACTION.read_text()
        releases = (
            (("Linux", "X64"), "x86_64-unknown-linux-musl",
             "45f1447fbe231e3037bde351ef70677dd212216c8d62ae7ca409fecc4d6acc89"),
            (("Linux", "ARM64"), "aarch64-unknown-linux-musl",
             "2b3284d5da3b46a47dc4229e75bb7b88ac4aa99c8d754fb7d2f84997e5a4354a"),
            (("macOS", "ARM64"), "aarch64-apple-darwin",
             "308184519b646f5125289e8515b36f6ca65a13a041923994aebe702348674e8e"),
        )
        for platform, target, checksum in releases:
            with self.subTest(platform=platform):
                _, exported, daemon = run_compile_cache_fixture(
                    action, platform=platform, archive=f"sccache-v0.18.0-{target}", checksum=checksum)
                self.assertIsNotNone(daemon)
                self.assertEqual(exported["RUSTC_WRAPPER"], "sccache")
                for download, server in ((22, 0), (0, 1)):
                    status, failed, attempted = run_compile_cache_fixture(
                        action, download, server, platform=platform)
                    self.assertEqual(status == 0, download == 0)
                    self.assertEqual(attempted is not None, download == 0)
                    self.assertNotIn("RUSTC_WRAPPER", failed)
        status, exported, daemon = run_compile_cache_fixture(action, platform=("Windows", "X64"))
        self.assertNotEqual(status, 0)
        self.assertNotIn("RUSTC_WRAPPER", exported)
        self.assertIsNone(daemon)

    def test_sccache_corrupt_archive_refuses_extraction_and_activation(self):
        source = CACHE_ACTION.read_text()
        script = textwrap.dedent(source.split("      run: |\n", 1)[1].split("    - name:", 1)[0])
        for platform in (("Linux", "X64"), ("Linux", "ARM64"), ("macOS", "ARM64")):
            with tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                shims = root / "shims"
                shims.mkdir()
                for name, body in {
                    "curl": '#!/bin/bash\nwhile [ "$1" != -o ]; do shift; done\nprintf corrupt > "$2"\n',
                    "tar": '#!/bin/bash\ntouch "$RUNNER_TEMP/extracted"\n',
                }.items():
                    path = shims / name
                    path.write_text(body)
                    path.chmod(0o700)
                result = subprocess.run(["bash", "-e", "-c", script], capture_output=True, text=True,
                                        timeout=10, env={**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
                                                        "RUNNER_OS": platform[0], "RUNNER_ARCH": platform[1], "RUNNER_TEMP": str(root),
                                                        "GITHUB_PATH": str(root / "path"), "GITHUB_ENV": str(root / "env")})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("FAILED", result.stdout)
                self.assertFalse((root / "extracted").exists())
                self.assertFalse((root / "path").exists())
                self.assertFalse((root / "env").exists())

    def test_unguarded_cache_paths_are_rejected(self):
        additions = [
            "      - uses: ./.github/actions/rust-compile-cache\n",
            "      - uses: actions/cache/restore@v4\n",
            "      - uses: actions/cache/save@v4\n",
            "      - uses: actions/setup-node@v4\n        with:\n          cache: npm\n",
            "      - uses: docker/build-push-action@v6\n        with:\n          cache-from: type=gha\n",
            "      - uses: docker/build-push-action@v6\n        with:\n          cache-to: type=gha,mode=max\n",
            "      - run: docker buildx build --cache-from type=gha .\n",
            '      - run: |\n          if [[ "${CI_USE_CACHE}" == "true" ]]; then\n'
            '            echo cached\n          else\n'
            '            docker buildx build --cache-from type=gha .\n          fi\n',
            "      - if: env.CI_USE_CACHE == 'true' || startsWith(github.ref, 'refs/tags/')\n"
            "        uses: actions/cache/restore@v4\n",
        ]
        for addition in additions:
            with self.subTest(step=addition):
                injected = SOURCE.replace("    steps:\n", "    steps:\n" + addition, 1)
                with self.assertRaises(AssertionError):
                    validate_cache_guards(injected)

    def test_only_develop_pushes_write_caches(self):
        validate_cache_writers(SOURCE)
        mutations = {
            "main writes": SOURCE.replace("github.ref == 'refs/heads/develop' }}",
                                          "(github.ref == 'refs/heads/develop' || github.ref == 'refs/heads/main') }}", 1),
            "every push writes": SOURCE.replace("github.event_name == 'push' && github.ref == 'refs/heads/develop' }}",
                                                "github.event_name == 'push' }}", 1),
            "a Rust cache saves on PRs": SOURCE.replace("save-if: ${{ env.CI_SAVE_CACHE == 'true' }}", "save-if: true", 1),
            "a save ignores the writer guard": SOURCE.replace(
                "if: env.CI_USE_CACHE == 'true' && env.CI_SAVE_CACHE == 'true' && steps.kubo-cache",
                "if: env.CI_USE_CACHE == 'true' && steps.kubo-cache", 1),
            "BuildKit restores remote layers": SOURCE.replace(
                "            --load \\\n", "            --cache-from type=gha \\\n            --load \\\n", 1),
        }
        for name, changed in mutations.items():
            self.assertNotEqual(changed, SOURCE, name)
            with self.subTest(mutation=name), self.assertRaises(AssertionError):
                validate_cache_writers(changed)

    def test_rust_cache_keys_bind_every_lockfile_and_the_toolchain(self):
        validate_rust_cache_keys(SOURCE)
        mutations = {
            "a prefix drops the lockfiles": SOURCE.replace("prefix-key: ${{ runner.os }}-capsules-bd" + RUST_CACHE_KEY,
                                                           "prefix-key: ${{ runner.os }}-capsules-bd", 1),
            "a prefix hashes one lockfile": SOURCE.replace("hashFiles('**/Cargo.lock', 'rust-toolchain.toml')",
                                                           "hashFiles('elastos/Cargo.lock', 'rust-toolchain.toml')", 1),
            "the compiler leaves the key": SOURCE.replace(
                "          cache-bin: false\n", "          cache-bin: false\n          add-rust-environment-hash-key: false\n", 1),
        }
        for name, changed in mutations.items():
            self.assertNotEqual(changed, SOURCE, name)
            with self.subTest(mutation=name), self.assertRaises(AssertionError):
                validate_rust_cache_keys(changed)

    def test_restored_inputs_are_verified_before_use(self):
        validate_restores_are_verified(SOURCE)
        kubo = "      - name: verify or download Kubo inputs\n        timeout-minutes: 10\n"
        mutations = {
            "Kubo check removed": SOURCE.replace(
                kubo + '        run: python3 scripts/ci-kubo-cache.py fetch "$KUBO_PLATFORM" "$KUBO_CACHE_DIR"\n', "", 1),
            "Kubo check skipped on a hit": SOURCE.replace(
                kubo, kubo + "        if: steps.kubo-cache.outputs.cache-hit != 'true'\n", 1),
            "engine used before its check": SOURCE.replace(
                "      - name: verify accepted ARM64 engine archive\n",
                "      - name: use engine\n        run: tar -tf \"$RUNNER_TEMP/llama-arm64-bundle\"/*.tar.gz\n"
                "      - name: verify accepted ARM64 engine archive\n", 1),
            "unknown restore": SOURCE.replace(
                "      - name: restore the pinned published release\n",
                "      - name: restore browser bundle\n        if: env.CI_USE_CACHE == 'true'\n"
                "        uses: actions/cache/restore@0057852bfaa89a56745cba8c7296529d2fc39830 # v4\n"
                "        with:\n          path: x\n          key: x\n"
                "      - name: restore the pinned published release\n", 1),
        }
        for name, changed in mutations.items():
            self.assertNotEqual(changed, SOURCE, name)
            with self.subTest(mutation=name), self.assertRaises(AssertionError):
                validate_restores_are_verified(changed)

    def test_github_runners_and_check_names_stay_fixed(self):
        expected = {
            "source-gate": ("source-gate", "ubuntu-24.04"),
            "engine-llama-arm64": ("engine-llama-arm64", "ubuntu-24.04-arm"),
            "engine-llama-amd64": ("engine-llama-amd64", "ubuntu-24.04"),
            "lint": ("lint", "ubuntu-24.04"),
            "test-elastos": ("test-elastos", "ubuntu-24.04"),
            "test-behaviour": ("test-behaviour", "ubuntu-24.04"),
            "test-capsules": ("test-capsules", "ubuntu-24.04"),
            "custody-harness-smoke": ("custody-harness-smoke", "ubuntu-24.04"),
            "release-upstream-paths": ("release-upstream-paths", "ubuntu-24.04"),
            "release-upstream-assets": ("release-upstream-assets (${{ matrix.platform }})", "${{ matrix.os }}"),
            "source-home-linux": ("source-home-linux (${{ matrix.os }})", "${{ matrix.os }}"),
            "source-home-linux-arm64": ("source-home-linux (${{ matrix.check_name || matrix.os }})", "${{ matrix.os }}"),
            "source-home-macos": ("source-home-macos", "macos-14"),
            "cancel-on-failure": ("cancel-on-failure", "ubuntu-24.04"),
            # Anders decides whether this becomes a required check (#320).
            "update-hop-linux": ("update-hop-linux", "ubuntu-24.04"),
            # Anders decides whether this becomes a required check (#311 D2).
            "cargo-audit": ("cargo-audit", "ubuntu-24.04"),
        }
        self.assertEqual(set(JOBS), set(expected))
        for job, (name, runner) in expected.items():
            self.assertEqual(field(JOBS[job], "name"), name)
            self.assertEqual(field(JOBS[job], "runs-on"), runner)
        self.assertEqual(field(JOBS["source-home-linux"], "os"),
                         "[ubuntu-24.04]")
        self.assertEqual(field(JOBS["source-home-linux-arm64"], "os"), "[ubuntu-22.04-arm]")
        self.assertIn("- os: ubuntu-22.04-arm\n            check_name: ubuntu-24.04-arm",
                      JOBS["source-home-linux-arm64"])
        self.assertIn("python3 scripts/ci-release-policy-test.py", SOURCE_GATE)

    def run_cancel_watcher(self, polls):
        """Run the watcher shell against successive job lists; return (status, cancels, polls used)."""
        step, = steps("cancel-on-failure")
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            shims, queue = root / "shims", root / "polls"
            shims.mkdir()
            queue.mkdir()
            for index, poll in enumerate(polls):
                (queue / f"{index:03}").write_text(json.dumps({"jobs": [
                    {"name": name, "status": status, "conclusion": conclusion}
                    for name, status, conclusion in poll]}))
            (shims / "sleep").write_text("#!/bin/bash\n")
            (shims / "gh").write_text(textwrap.dedent("""\
                #!/bin/bash
                if [ "$2" = -X ]; then echo "$4" >> "$CANCEL_LOG"; exit 0; fi
                next="$(ls "$POLL_DIR" | sort | sed -n 1p)"
                [ -n "$next" ] || exit 9
                jq -r "$4" "$POLL_DIR/$next" && rm "$POLL_DIR/$next"
                """))
            for shim in shims.iterdir():
                shim.chmod(0o700)
            result = subprocess.run(["bash", "-c", script], capture_output=True, text=True, timeout=10,
                                    env={**os.environ, "PATH": f"{shims}{os.pathsep}{os.environ['PATH']}",
                                         "GITHUB_REPOSITORY": "Elacity/elastos-runtime", "GITHUB_RUN_ID": "7",
                                         "GITHUB_RUN_ATTEMPT": "1", "POLL_DIR": str(queue),
                                         "CANCEL_LOG": str(root / "cancels")})
            cancels = (root / "cancels").read_text().splitlines() if (root / "cancels").exists() else []
            return result.returncode, cancels, len(polls) - len(list(queue.iterdir()))

    def test_pull_request_runs_cancel_at_the_first_failed_job(self):
        watcher = JOBS["cancel-on-failure"]
        self.assertNotIn("actions/checkout@", watcher)
        self.assertEqual(watcher.split("    permissions:\n", 1)[1].split("    steps:\n", 1)[0], "      actions: write\n")
        for event, ref, ref_type, override, _ in CASES:
            for head in ("Elacity/elastos-runtime", "someone/fork"):
                context = {"github.event_name": event, "github.ref": ref, "github.repository": "Elacity/elastos-runtime",
                           "github.event.pull_request.head.repo.full_name": head}
                self.assertEqual(job_runs("cancel-on-failure", context),
                                 event == "pull_request" and head == "Elacity/elastos-runtime", (event, head))
        watch = ("cancel-on-failure", "in_progress", None)
        gate = ("source-gate", "completed", "success")
        macos = ("source-home-macos", "in_progress", None)
        for conclusion in ("failure", "timed_out"):
            status, cancels, used = self.run_cancel_watcher(
                [[watch, gate, macos], [watch, gate, ("lint", "completed", conclusion), macos]])
            self.assertEqual((status, cancels, used), (0, ["repos/Elacity/elastos-runtime/actions/runs/7/cancel"], 2))
        # A completed gate before dependent jobs appear is not the end of the run.
        done = [watch, gate, ("source-home-macos", "completed", "success"), ("lint", "completed", "cancelled")]
        status, cancels, used = self.run_cancel_watcher([[watch, gate], [watch, gate, macos], done, done])
        self.assertEqual((status, cancels, used), (0, [], 4))

    def test_disposable_refusals_run_on_mac_build_without_operator_inputs(self):
        mac_steps = steps("source-home-macos")
        names = [step.splitlines()[0] for step in mac_steps]
        generate = names.index("name: generate disposable signed install and update fixture")
        prove = names.index("name: prove installed Home account and System update with Carrier hop and refusals")
        build = names.index("name: build two actual Runtime versions")
        capacity = names.index("name: reserve hosted Mac build and fixture capacity")
        self.assertLess(capacity, names.index("name: source-home into isolated MAC_TEST_HOME"))
        self.assertIn("prepare-ci-disk", mac_steps[capacity])
        self.assertEqual(build + 1, generate)
        self.assertIn("build-ci-hop", mac_steps[build])
        self.assertIn("assert os.environ['CI_MAC_BUILD_DIR_FRESH'] == 'true'", mac_steps[build])
        self.assertIn("'initially_absent': True", mac_steps[build])
        self.assertIn("assert directory == pathlib.Path(os.environ['RUNNER_TEMP']) / 'source-home-macos-build'", mac_steps[build])
        self.assertIn("assert target == pathlib.Path(os.environ['RUNNER_TEMP']) / 'source-home-macos-target'", mac_steps[build])
        self.assertIn("'target_directory': str(target)", mac_steps[build])
        self.assertIn('--runtime "$CARGO_TARGET_DIR/release/elastos" --previous "$RUNNER_TEMP/previous-release"', mac_steps[build])
        setup = mac_steps[names.index("name: source-home into isolated MAC_TEST_HOME")]
        self.assertIn('echo "ELASTOS_RELEASE_VERSION=$ELASTOS_RELEASE_VERSION" >> "$GITHUB_ENV"', setup)
        # The old side is the pinned published release; the cache serves it and a miss reads the seed.
        restore = names.index("name: restore the pinned published release")
        fetch = names.index("name: fetch the pinned published release")
        save = names.index("name: save the pinned published release")
        self.assertEqual((fetch, save, build), (restore + 1, restore + 2, restore + 3))
        key = "previous-release-${{ hashFiles('scripts/update-hop-previous-release.json') }}"
        self.assertEqual(field(mac_steps[restore], "key"), key)
        self.assertEqual(field(mac_steps[save], "key"), key)
        for step in (mac_steps[restore], mac_steps[save]):
            self.assertRegex(field(step, "uses"), r"^actions/cache/(?:restore|save)@[0-9a-f]{40} # v")
        self.assertIn('fetch-previous-release \\\n            "$RUNNER_TEMP/previous-release-cache" "$RUNNER_TEMP/previous-release"', mac_steps[fetch])
        self.assertNotIn("if:", mac_steps[fetch])
        self.assertIn('--previous "$RUNNER_TEMP/previous-release"', mac_steps[generate])
        self.assertLess(names.index("name: source-home into isolated MAC_TEST_HOME"), generate)
        self.assertEqual(prove, generate + 1)
        self.assertNotIn("if:", mac_steps[generate])
        self.assertNotIn("if:", mac_steps[prove])
        self.assertIn("generate-ci-hop", mac_steps[generate])
        self.assertIn('--runtime "$CI_HOP_ROOT/build-inputs/elastos-old"', mac_steps[generate])
        self.assertIn('--next-runtime "$CI_HOP_ROOT/build-inputs/elastos-new"', mac_steps[generate])
        self.assertIn('--system-runtime "$CI_HOP_ROOT/build-inputs/elastos-system"', mac_steps[generate])
        self.assertIn('--build-receipt "$CI_HOP_ROOT/build-inputs/build.json"', mac_steps[generate])
        self.assertIn('--support-home "$RUNNER_TEMP/elastos-mac-test-home/Library/Application Support/elastos"', mac_steps[generate])
        self.assertIn("ELASTOS_CI_FIXTURE_SCOPE: ci-rehearsal", mac_steps[prove])
        self.assertIn('ELASTOS_CI_REQUIRE_REAL_RUNTIME: "1"', mac_steps[prove])
        self.assertIn('python3 scripts/update-hop-compare.py run "$CI_HOP_ROOT/package/fixture.json"', mac_steps[prove])
        verdict = mac_steps[prove + 1]
        self.assertIn('update-hop-compare.py check-result "$CI_HOP_ROOT/package/results/result.json"', verdict)
        self.assertNotIn("if:", verdict)
        for step in mac_steps[generate:prove + 1]:
            self.assertNotIn("FIXTURE_ARTIFACT_ID", step)
            self.assertNotIn("GH_TOKEN", step)
            self.assertNotIn("inputs.", step)
        upload = mac_steps[names.index("name: retain the safe CI hop receipt")]
        self.assertIn("${{ env.CI_HOP_ROOT }}/package/results/result.json", upload)
        self.assertEqual(field(upload, "name"), "retain the safe CI hop receipt")
        self.assertIn("name: macos-cli-update-hop-receipt", upload)
        self.assertIn("if: always()", upload)
        cleanup = mac_steps[names.index("name: remove stopped CI hop fixture files")]
        self.assertIn("get('cleanup', {}).get('passed')", cleanup)
        self.assertIn("shutil.rmtree(root)", cleanup)

    def test_linux_hop_installs_the_pinned_release_and_proves_update_refusals_and_undo(self):
        linux_steps = steps("update-hop-linux")
        names = [step.splitlines()[0] for step in linux_steps]
        order = [names.index("name: " + name) for name in (
            "source-home into isolated HOME", "prepare installed Home browser", "install the pinned published release",
            "build two actual Runtime versions", "generate disposable signed install and update fixture",
            "prove installed Home account and System update with Carrier hop and refusals",
            "require the update from the published release, no new provider warnings and Undo")]
        self.assertEqual(order, sorted(order))
        self.assertEqual(order[2:], list(range(order[2], order[2] + 5)))
        source_home, _, install, build, generate, prove, verdict = (linux_steps[index] for index in order)
        # The seed's HTTP route does not serve the Linux Runtime; a Home installs the pin over Carrier.
        self.assertIn('install-previous-release \\\n            "$RUNNER_TEMP/previous-release-home" "$RUNNER_TEMP/previous-release"', install)
        self.assertIn('export XDG_DATA_HOME="${SOURCE_HOME}/.local/share"', source_home)
        self.assertIn('export ELASTOS_RELEASE_VERSION=', source_home)
        self.assertIn('--runtime "$CARGO_TARGET_DIR/release/elastos" --previous "$RUNNER_TEMP/previous-release"', build)
        self.assertIn('--support-home "$RUNNER_TEMP/elastos-source-home/.local/share/elastos"', generate)
        self.assertIn("ELASTOS_CI_FIXTURE_SCOPE: ci-rehearsal", prove)
        self.assertIn('ELASTOS_CI_REQUIRE_REAL_RUNTIME: "1"', prove)
        self.assertIn('update-hop-compare.py check-result "$CI_HOP_ROOT/package/results/result.json"', verdict)
        for step in (install, build, generate, prove, verdict):
            self.assertNotIn("if:", step)
            self.assertNotRegex(step, r"FIXTURE_ARTIFACT_ID|GH_TOKEN|inputs\.")
        upload = linux_steps[names.index("name: retain the safe CI hop receipt")]
        self.assertIn("if: always()", upload)
        self.assertIn("name: linux-cli-update-hop-receipt", upload)

    def test_engine_cache_has_exact_recipe_key_and_develop_only_writers(self):
        restore, = [step for step in steps("engine-llama-arm64") if "actions/cache/restore@" in step]
        save, = [step for step in steps("engine-llama-arm64") if "actions/cache/save@" in step]
        self.assertEqual(field(restore, "key"), "${{ steps.engine-recipe.outputs.cache-key }}")
        self.assertEqual(field(save, "key"), "${{ steps.engine-cache.outputs.cache-primary-key }}")
        self.assertNotIn("restore-keys:", restore)
        for event, ref, ref_type, override, cached in CASES:
            for hit in ("true", "false"):
                context = {"github.event_name": event, "github.ref": ref,
                           "env.CI_USE_CACHE": str(cached).lower(),
                           "steps.engine-cache.outputs.cache-hit": hit}
                self.assertEqual(evaluate(field(save, "if"), context),
                                 cached and event == "push" and ref == "refs/heads/develop" and hit == "false")
        self.assertEqual(field(JOBS["engine-llama-arm64"], "image"),
                         field(JOBS["engine-llama-arm64"], "BUILD_CONTAINER_IMAGE"))
        self.assertIn('apt-get install -y -qq --no-install-recommends "${tools[@]}"',
                      JOBS["engine-llama-arm64"])
        self.assertIn('read -r -a tools <<< "$ENGINE_BUILD_TOOLS"', JOBS["engine-llama-arm64"])

    def test_engine_recipe_key_changes_with_source_container_or_tools(self):
        recipe, = [step for step in steps("engine-llama-arm64") if "id: engine-recipe" in step]
        script = textwrap.dedent(recipe.split("        run: |\n", 1)[1])
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "scripts/build/build-llama-server-bundle.sh"
            source.parent.mkdir(parents=True)
            source.write_text("original flags")
            helper = root / "scripts/verify-linux-engine.py"
            helper.write_text("original verifier")
            output = root / "output"
            env = {**os.environ, "GITHUB_OUTPUT": str(output),
                   "BUILD_CONTAINER_IMAGE": "pinned-image", "ENGINE_BUILD_TOOLS": "pinned-tools"}
            def key():
                output.write_text("")
                subprocess.run(["bash", "-euo", "pipefail", "-c", script],
                               cwd=root, env=env, check=True)
                return output.read_text()
            original = key()
            helper.write_text("changed verifier")
            self.assertNotEqual(original, key())
            helper.write_text("original verifier")
            self.assertRegex(original, r"^cache-key=develop-llama-arm64-[0-9a-f]{64}\n$")
            source.write_text("changed flags")
            self.assertNotEqual(original, key())
            source.write_text("original flags")
            for name in ("BUILD_CONTAINER_IMAGE", "ENGINE_BUILD_TOOLS"):
                previous = env[name]
                env[name] += " changed"
                self.assertNotEqual(original, key())
                env[name] = previous
            env["RECIPE_COMMIT"] = "changed checkout"
            self.assertEqual(original, key())

    def test_engine_build_receipt_uses_the_checked_out_recipe_commit(self):
        build, = [step for step in steps("engine-llama-arm64") if "id: engine-build" in step]
        self.assertIn('RECIPE_COMMIT="$(git -c safe.directory="$GITHUB_WORKSPACE" rev-parse HEAD)"', build)
        self.assertIn('export RECIPE_COMMIT\n', build)
        self.assertNotIn("RECIPE_COMMIT: ${{ github.sha }}", JOBS["engine-llama-arm64"])

    def test_engine_consumers_gate_the_current_run_input_with_shared_pin(self):
        self.assertEqual(field(JOBS["source-home-linux"], "needs"), "[source-gate, engine-llama-amd64]")
        self.assertEqual(field(JOBS["source-home-linux-arm64"], "needs"),
                         "[source-gate, engine-llama-arm64]")
        download, = [step for step in steps("source-home-linux-arm64") if "actions/download-artifact@" in step and "name: llama-arm64-bundle" in step]
        self.assertIn("name: llama-arm64-bundle", download)
        self.assertEqual(field(download, "if"), "matrix.os == 'ubuntu-22.04-arm'")
        self.assertNotIn("run-id:", download)
        self.assertNotIn("36501810782", SOURCE)
        verify = 'bash scripts/build/build-llama-server-bundle.sh --verify-archive "$RUNNER_TEMP/llama-arm64-bundle"'
        for job in ("engine-llama-arm64", "source-home-linux-arm64"):
            self.assertIn(verify, "\n".join(steps(job)))
        consumer_gate, = [step for step in steps("source-home-linux-arm64") if verify in step]
        self.assertEqual(field(consumer_gate, "if"), "matrix.os == 'ubuntu-22.04-arm'")
        producer = steps("engine-llama-arm64")
        gate = next(i for i, step in enumerate(producer) if verify in step)
        saving = next(i for i, step in enumerate(producer) if "actions/cache/save@" in step)
        self.assertLess(gate, saving)
        self.assertNotIn("always()", producer[saving])
        pin = field(SOURCE, "CI_LLAMA_ARM64_SHA256")
        self.assertRegex(pin, r"^[0-9a-f]{64}$")
        manifest = json.loads((WORKFLOW.parents[2] / "components.json").read_text())
        component = manifest["external"]["llama-server"]["platforms"]["linux-arm64"]
        if "checksum" in component:
            self.assertEqual(component["checksum"], "sha256:" + pin)

    def test_engine_archive_rejects_missing_malformed_and_wrong_ci_pins(self):
        builder = WORKFLOW.parents[2] / "scripts/build/build-llama-server-bundle.sh"
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive_path = root / "llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz"
            with tarfile.open(archive_path, "w:gz", format=tarfile.GNU_FORMAT) as archive:
                for name in ("llama-server", "libllama.so"):
                    member = tarfile.TarInfo("llama-b10516/" + name)
                    data = b"\x7fELFinert fixture"
                    member.size = len(data)
                    member.mode = 0o755
                    archive.addfile(member, io.BytesIO(data))
            tools = root / "tools"
            tools.mkdir()
            (tools / "readelf").write_text('#!/bin/sh\ncase "$1" in -h) printf "Class: ELF64\\nMachine: AArch64\\n" ;; -d) echo \'(RUNPATH) [$ORIGIN]\'; echo "(NEEDED) [libc.so.6]" ;; *) echo GLIBC_2.35 ;; esac\n')
            (tools / "ldd").write_text('#!/bin/sh\necho "resolved dependencies"\n')
            for path in tools.iterdir():
                path.chmod(0o755)
            correct = hashlib.sha256(archive_path.read_bytes()).hexdigest()
            for checksum in (correct, "0" * 64, "", "untrusted", "md5:" + "0" * 64):
                result = subprocess.run(["bash", str(builder), "--verify-archive", directory],
                                        env={**os.environ, "CI_LLAMA_ARM64_SHA256": checksum,
                                             "PATH": str(tools) + os.pathsep + os.environ["PATH"]},
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, checksum == correct, result.stderr)

    def test_linux_engine_producers_share_pins_and_verify_every_archive_member(self):
        recipes = json.loads((WORKFLOW.parents[2] / "scripts/release-upstream-recipes.json").read_text())["recipes"]
        release_jobs = jobs((WORKFLOW.parent / "release-package.yml").read_text())
        for arch in ("amd64", "arm64"):
            recipe = next(row for row in recipes if row["component"] == "llama-server" and row["platform"] == "linux-" + arch)
            self.assertEqual(recipe["source"]["path"], "@llama-" + arch + "-bundle")
            self.assertEqual("sha256:" + field(SOURCE, "CI_LLAMA_" + arch.upper() + "_SHA256"), recipe["source"]["checksum"])
            job = "engine-llama-" + arch
            for workflow_jobs in (JOBS, release_jobs):
                producer = steps(job, workflow_jobs)
                build, = [step for step in producer if "id: engine-build\n" in step]
                verify, = [step for step in producer if "--verify-archive" in step]
                self.assertLess(producer.index(build), producer.index(verify))
                self.assertNotIn("if:", verify)
                self.assertIn("--verify-archive", verify)
                self.assertIn("cat scripts/verify-linux-engine.py", workflow_jobs[job])
                if arch == "amd64":
                    self.assertIn("--platform linux-amd64", build)
                    self.assertIn("--platform linux-amd64", verify)
        builder = (WORKFLOW.parents[2] / "scripts/build/build-llama-server-bundle.sh").read_text()
        self.assertIn("-DGGML_OPENMP=OFF", builder)
        self.assertIn('--archive "$2/llama-b10516-bin-ubuntu22.04-$architecture-cpu.tar.gz"', builder)
        prepare = (WORKFLOW.parents[2] / "scripts/prepare-release-platform.sh").read_text()
        self.assertIn('--archive "$STAGING/artifacts/llama-server-$SETUP_PLATFORM.tar.gz"', prepare)

    def test_engine_elf_validator_refuses_wrong_architecture_libraries_and_symbols(self):
        import importlib.util
        spec = importlib.util.spec_from_file_location("engine_validator", WORKFLOW.parents[2] / "scripts/verify-linux-engine.py")
        validator = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(validator)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("llama-server", "libllama.so"):
                (root / name).write_bytes(b"\x7fELFfixture")
            cases = [("AArch64", "$ORIGIN", "libc.so.6", "GLIBC_2.35", True),
                     ("Advanced Micro Devices X86-64", "$ORIGIN", "libc.so.6", "GLIBC_2.35", False),
                     ("AArch64", "/host", "libc.so.6", "GLIBC_2.35", False),
                     ("AArch64", "$ORIGIN", "libmissing.so", "GLIBC_2.35", False),
                     ("AArch64", "$ORIGIN", "libc.so.6", "GLIBC_2.36", False)]
            for machine, runpath, library, version, accepted in cases:
                def inspect(args, **kwargs):
                    if args[0] == "ldd":
                        return "resolved dependencies"
                    if args[1] == "-h":
                        return f"Class: ELF64\nMachine: {machine}\n"
                    if args[1] == "-d":
                        return f"(RUNPATH) [{runpath}]\n(NEEDED) [{library}]\n"
                    return version
                with mock.patch("sys.argv", ["validator", directory]), \
                     mock.patch("subprocess.check_output", side_effect=inspect), \
                     mock.patch("builtins.print"):
                    if accepted:
                        validator.verify_bundle(root, "arm64")
                    else:
                        with self.assertRaises(ValueError):
                            validator.verify_bundle(root, "arm64")


class CustodyKuboDownloadTests(unittest.TestCase):
    """Execute the Dockerfile's download gate with local transport fixtures."""

    def setUp(self):
        self.source = (WORKFLOW.parents[2] / "deploy/custody-host/Dockerfile").read_text()
        self.version = re.search(r"(?m)^ARG KUBO_VERSION=(\S+)$", self.source)[1]
        self.script = self.source.split("ARG KUBO_VERSION=", 1)[1].split("<<'EOF'\n", 1)[1].split("\nEOF", 1)[0]
        self.pins = dict(re.findall(r'(amd64|arm64)\) kubo_sha256="([0-9a-f]{64})"', self.script))
        self.assertEqual(self.version, "v0.42.0")
        self.assertEqual(self.pins, {
            "amd64": "284145534168b51fe980f73c90f0ce84b55ca293034836b2ba8ea8f93435116e",
            "arm64": "edc6f485ab623f9327bf2ad7aa7a29d84c87c72f1e2584376a77240134a96e69"})

    def run_gate(self, arch="amd64", primary="valid", mirror="valid"):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "fixture.tar.gz"
            binary = b"local Kubo download fixture"
            with tarfile.open(archive, "w:gz") as tar:
                entry = tarfile.TarInfo("kubo/ipfs")
                entry.size = len(binary)
                tar.addfile(entry, io.BytesIO(binary))
            fixture_pin = hashlib.sha256(archive.read_bytes()).hexdigest()
            # Only this in-memory test copy uses the small fixture checksum.
            # setUp separately enforces both exact production pins above.
            script = self.script
            for pin in self.pins.values():
                script = script.replace(pin, fixture_pin)
            script = script.replace("/tmp", str(root))
            shims = root / "shims"
            shims.mkdir()
            curl = shims / "curl"
            curl.write_text("#!" + sys.executable + "\n" + textwrap.dedent('''\
                import json, os, pathlib, sys
                args = sys.argv[1:]
                out = pathlib.Path(args[args.index("-o") + 1])
                url = args[-1]
                route = "PRIMARY" if url.startswith("https://dist.ipfs.tech/") else "MIRROR"
                existed = out.exists()
                with open(os.environ["FIXTURE_EVENTS"], "a") as log:
                    log.write(json.dumps({"args": args, "route": route, "existing": existed}) + "\\n")
                response = os.environ["FIXTURE_" + route]
                out.write_bytes(pathlib.Path(os.environ["FIXTURE_ARCHIVE"]).read_bytes()
                                if response == "valid" else b"failed or corrupt transport bytes")
                sys.exit(22 if response == "failed" else 0)
                '''))
            curl.chmod(0o700)
            strip = shims / "strip"
            strip.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$FIXTURE_STRIP"\n')
            strip.chmod(0o700)
            tar = shims / "tar"
            tar.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> "$FIXTURE_TAR"\nexec /usr/bin/tar "$@"\n')
            tar.chmod(0o700)
            env = {**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
                   "KUBO_VERSION": self.version, "TARGETARCH": arch,
                   "FIXTURE_PRIMARY": primary, "FIXTURE_MIRROR": mirror,
                   "FIXTURE_EVENTS": str(root / "events"), "FIXTURE_STRIP": str(root / "strip-log"),
                   "FIXTURE_TAR": str(root / "tar-log"),
                   "FIXTURE_ARCHIVE": str(archive)}
            result = subprocess.run(["/bin/sh", "-c", script], env=env,
                                    capture_output=True, text=True, timeout=10)
            events = [json.loads(line) for line in (root / "events").read_text().splitlines()] if (root / "events").exists() else []
            installed = (root / "kubo-binary").read_bytes() if (root / "kubo-binary").exists() else None
            stripped = (root / "strip-log").exists()
            archive_left = (root / f"kubo_{self.version}_linux-{arch}.tar.gz").exists()
            mode = (root / "kubo-binary").stat().st_mode & 0o777 if installed else None
            self.assertEqual((root / "tar-log").exists(), result.returncode == 0,
                             "refused input must stop before archive extraction")
            return result, events, installed, stripped, archive_left, mode

    def test_primary_and_byte_identical_fallback_keep_platform_and_install_gate(self):
        for arch in ("amd64", "arm64"):
            for primary in ("valid", "failed"):
                with self.subTest(arch=arch, primary=primary):
                    result, events, installed, stripped, archive_left, mode = self.run_gate(arch, primary)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertEqual(installed, b"local Kubo download fixture")
                    self.assertTrue(stripped)
                    self.assertFalse(archive_left)
                    self.assertEqual(mode, 0o755)
                    routes = ["PRIMARY"] if primary == "valid" else ["PRIMARY", "MIRROR"]
                    self.assertEqual([event["route"] for event in events], routes)
                    tarball = f"kubo_{self.version}_linux-{arch}.tar.gz"
                    urls = [f"https://dist.ipfs.tech/kubo/{self.version}/{tarball}",
                            f"https://github.com/ipfs/kubo/releases/download/{self.version}/{tarball}"]
                    self.assertEqual([event["args"][-1] for event in events], urls[:len(events)])
                    for event in events:
                        self.assertEqual(event["args"][:13], ["-fsSL", "--connect-timeout", "15", "--max-time", "90",
                            "--retry", "2", "--retry-all-errors", "--retry-delay", "5", "--retry-max-time", "200", "-o"])
                    if primary == "failed":
                        self.assertFalse(events[1]["existing"], "failed primary bytes must be removed before fallback")

    def test_bad_checksum_or_unreachable_routes_refuse_extraction_and_install(self):
        for arch in ("amd64", "arm64"):
            for primary, mirror, routes in (("corrupt", "valid", ["PRIMARY"]),
                                             ("failed", "corrupt", ["PRIMARY", "MIRROR"]),
                                             ("failed", "failed", ["PRIMARY", "MIRROR"])):
                with self.subTest(arch=arch, primary=primary, mirror=mirror):
                    result, events, installed, stripped, _, _ = self.run_gate(arch, primary, mirror)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual([event["route"] for event in events], routes)
                    self.assertIsNone(installed)
                    self.assertFalse(stripped)
                    if mirror != "failed":
                        self.assertIn("FAILED", result.stdout)

    def test_unsupported_architecture_refuses_every_download(self):
        result, events, installed, stripped, _, _ = self.run_gate("riscv64")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unsupported TARGETARCH", result.stderr)
        self.assertEqual(events, [])
        self.assertIsNone(installed)
        self.assertFalse(stripped)


class InstalledJourneyTests(unittest.TestCase):
    def fixture(self, root, platform):
        root = root.resolve()
        home = root / "home"
        host = "darwin-arm64" if platform == "macos" else "linux-amd64"
        data = home / ("Library/Application Support/elastos" if platform == "macos"
                       else ".local/share/elastos")
        evidence = root / "evidence"
        (data / "bin").mkdir(parents=True)
        (data / "receipts").mkdir()
        (home / "model-inputs/inputs").mkdir(parents=True)
        evidence.mkdir()
        runtime = data / "bin/elastos"
        runtime.write_bytes(b"installed fixture Runtime")
        provider = data / "bin/model-provider"
        provider.write_bytes(b"installed fixture model provider")
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        recipe, = [row for row in json.loads((WORKFLOW.parents[2] / "scripts/release-upstream-recipes.json").read_text())["recipes"]
                   if row["component"] == "llama-server" and row["platform"] == host]
        engine_version = json.loads((WORKFLOW.parents[2] / "components.json").read_text())["external"]["llama-server"]["version"]
        bundle = data / recipe["install_path"]
        bundle.mkdir(parents=True)
        binary = bundle / recipe["binary_path"]
        binary.write_bytes(b"synthetic engine bytes; never executed")
        binary.chmod(0o700)
        provenance = {"recipe_sha256": hashlib.sha256(journey["UPSTREAM"]["canonical"](journey["UPSTREAM"]["public_recipe"](recipe))).hexdigest(),
                      "upstream": journey["UPSTREAM"]["source_record"](recipe["source"])}
        (bundle / "PROVENANCE.json").write_text(json.dumps(provenance))
        entries = [{"path": path.relative_to(bundle).as_posix(), "type": "file",
                    "sha256": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()}
                   for path in (binary, bundle / "PROVENANCE.json")]
        archive = "sha256:" + "a" * 64
        (bundle / ".elastos-engine.json").write_text(json.dumps({
            "schema": "elastos.local-model-engine/v2", "platform": host, "version": engine_version,
            "archive_sha256": archive, "entries": entries}))
        (data / "bin/llama-server").symlink_to(binary)
        manifest = {"external": {
            "model-provider": {"platforms": {host: {"checksum": "sha256:" + hashlib.sha256(provider.read_bytes()).hexdigest()}}},
            "llama-server": {"version": engine_version, "platforms": {host: {
                "install_path": recipe["install_path"], "binary_path": recipe["binary_path"], "checksum": archive}}}}}
        (data / "components.json").write_text(json.dumps(manifest))
        runtime_sha = "sha256:" + hashlib.sha256(runtime.read_bytes()).hexdigest()
        receipt = {"platform": host, "source": {"commit": "c" * 40, "tree": "d" * 40, "clean": True},
                   "runtime": {"built_sha256": runtime_sha, "installed_sha256": runtime_sha, "parity": True},
                   "components_sha256": "sha256:" + hashlib.sha256((data / "components.json").read_bytes()).hexdigest()}
        (data / "receipts/source-home-installation.json").write_text(json.dumps(receipt))
        return home, data, evidence, receipt

    PACKAGE_MULTIHASH = b"\x12\x20" + hashlib.sha256(b"fixture package root").digest()
    PACKAGE_CID = "b" + base64.b32encode(b"\x01\x70" + PACKAGE_MULTIHASH).decode().lower().rstrip("=")

    def consumer_kubo(self, data, holds_root=False):
        # Kubo's flatfs layout: blocks/<next-to-last 2>/<base32 multihash>.data.
        blocks = data / "ipfs-repo/blocks"
        blocks.mkdir(parents=True)
        (blocks / "SHARDING").write_text("/repo/flatfs/shard/v1/next-to-last/2\n")
        if holds_root:
            key = base64.b32encode(self.PACKAGE_MULTIHASH).decode().rstrip("=")
            (blocks / key[-3:-1]).mkdir()
            (blocks / key[-3:-1] / f"{key}.data").write_bytes(b"package root")

    def execute(self, home, data, evidence, available=20 * 1024 ** 3, carrier=False, consumer_holds_root=False, memory_gib=0):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        child = mock.Mock(pid=12345)
        child.poll.return_value = None
        response = mock.MagicMock()
        response.__enter__.return_value.status = 200
        self.holder = mock.Mock(return_value=(mock.Mock(pid=23456), {"holder_did": "did:key:holder"}))
        self.finite = mock.Mock()

        def node_journey(*args, **kwargs):
            if args[0][1] == "scripts/ci-model-package.mjs":
                (evidence / "package.json").write_text(json.dumps({"cid": self.PACKAGE_CID}))
                return
            self.consumer_kubo(data, consumer_holds_root)
            stages = [("run_started", 2), ("engine_ready", 400), ("generation_started", 500),
                      ("first_delta", 600), ("stream_completed", 1000),
                      ("generation_completed", 1020), ("terminal_applied", 1030)]
            (home / "journey-runtime.private.log").write_text("".join(
                f"[model-provider] local timing stage={name} elapsed_ms={value}\n" for name, value in stages)
                + "[model-provider] local acknowledgement kind=delta outcome=applied elapsed_ms=1020 duration_ms=20\n"
                + "[model-provider] local acknowledgement kind=terminal outcome=applied elapsed_ms=1030 duration_ms=10\n")
            (evidence / "home-journey.json").write_text(json.dumps({"results": {
                "home_screenshots": "passed", "model_package_admission": "passed",
                "installed_runtime_reply": "passed"}}))

        with mock.patch.dict(os.environ, {"PATH": "/fixture-tools"}, clear=True), \
                mock.patch.object(subprocess, "check_output", side_effect=lambda args, **_: (
                    ("c" if args[-1] == "HEAD" else "d") * 40 + "\n" if args[0] == "git" else "")), \
                mock.patch.object(subprocess, "Popen", return_value=child) as gateway, \
                mock.patch.object(subprocess, "run", side_effect=node_journey) as node, \
                mock.patch("urllib.request.urlopen", return_value=response), \
                mock.patch.object(os, "killpg") as stop, \
                mock.patch.dict(journey["run"].__globals__, {"disk_observation": lambda _: {
                    "capacity_bytes": 100 * 1024 ** 3, "available_bytes": available},
                    "process_rows": lambda: {},
                    "cleanup_runtime": lambda *_, **__: {"status": "passed", "before": {}, "after": {}},
                    "start_holder": self.holder, "FiniteRuntime": self.finite}):
            try:
                journey["run"](home, data, evidence, carrier=carrier, memory_gib=memory_gib)
            finally:
                self.gateway, self.node, self.stop = gateway, node, stop
        return child

    def test_stale_ui_receipt_is_refused_before_fixture_or_runtime_start(self):
        for kind in ("receipt", "dangling_link"):
            for memory_gib in (0, 1, 4):
                with self.subTest(kind=kind, memory_gib=memory_gib), tempfile.TemporaryDirectory() as temp:
                    home, data, evidence, _ = self.fixture(Path(temp), "linux")
                    old = json.dumps({"results": {"installed_runtime_reply": "passed"}})
                    path = evidence / "home-journey.json"
                    if kind == "receipt":
                        path.write_text(old)
                    else:
                        path.symlink_to("never-created.json")
                    with self.assertRaisesRegex(RuntimeError, "requires fresh UI evidence"):
                        self.execute(home, data, evidence, memory_gib=memory_gib)
                    self.gateway.assert_not_called()
                    self.node.assert_not_called()
                    self.finite.assert_not_called()
                    if kind == "receipt":
                        self.assertEqual(path.read_text(), old)
                    else:
                        self.assertTrue(path.is_symlink())
                        self.assertFalse(path.exists())

    def test_gateway_and_node_share_the_installed_fixture_root(self):
        for platform in ("macos", "linux"):
            with self.subTest(platform=platform), tempfile.TemporaryDirectory() as temp:
                home, data, evidence, _ = self.fixture(Path(temp), platform)
                child = self.execute(home, data, evidence)
                self.assertEqual(self.gateway.call_args.args[0][:2],
                                 [str(data / "bin/elastos"), "gateway"])
                self.assertEqual(self.node.call_args.args[0][:2],
                                 ["node", "scripts/ci-installed-home-journey.mjs"])
                for process in (self.gateway, self.node):
                    env = process.call_args.kwargs["env"]
                    self.assertEqual(env["HOME"], str(home))
                    self.assertEqual(Path(env["XDG_DATA_HOME"]) / "elastos", data)
                    self.assertEqual(env["PATH"], "/fixture-tools")
                    self.assertEqual(env["ELASTOS_MODEL_TIMING_DIAGNOSTICS"], "1")
                self.assertEqual(self.node.call_count, 2)
                package = self.node.call_args_list[0].args[0]
                self.assertEqual(package[1], "scripts/ci-model-package.mjs")
                self.assertEqual(package[2:4], [str(data), str(data)])
                self.holder.assert_not_called()
                record = json.loads((evidence / "installed-journeys.json").read_text())
                self.assertEqual(record["installed_model_provider_sha256"],
                                 hashlib.sha256((data / "bin/model-provider").read_bytes()).hexdigest())
                self.assertNotIn("carrier_get", record["results"])

    def test_carrier_get_places_the_package_only_on_a_separate_holder_home(self):
        with tempfile.TemporaryDirectory() as temp:
            home, data, evidence, _ = self.fixture(Path(temp), "linux")
            self.execute(home, data, evidence, carrier=True)
            package = self.node.call_args_list[0].args[0]
            # The package goes only to the separate holder Home; the consumer gets the catalogue.
            holder_home = home.parent / "home-holder"
            holder_data = holder_home / data.relative_to(home)
            self.assertEqual(package[2:4], [str(holder_data), str(data)])
            self.assertEqual(self.holder.call_args.args[:3], (holder_home, holder_data, data))
            self.assertEqual(self.gateway.call_args.args[0][:2], [str(data / "bin/elastos"), "gateway"])
            record = json.loads((evidence / "installed-journeys.json").read_text())
            self.assertEqual(record["results"]["carrier_get"], "passed")
            self.assertEqual(record["package_holder"], {"holder_did": "did:key:holder"})

    def test_get_refuses_a_package_present_in_the_consumer_kubo(self):
        with tempfile.TemporaryDirectory() as temp:
            home, data, evidence, _ = self.fixture(Path(temp), "linux")
            with self.assertRaisesRegex(RuntimeError, "Carrier-only package delivery"):
                self.execute(home, data, evidence, carrier=True, consumer_holds_root=True)
            record = json.loads((evidence / "installed-journeys.json").read_text())
            self.assertEqual(record["consumer_package_root_block"], "present")
            self.assertEqual(record["results"]["carrier_get"], "failed")

    def test_get_refuses_a_consumer_that_already_holds_a_kubo_repository(self):
        with tempfile.TemporaryDirectory() as temp:
            home, data, evidence, _ = self.fixture(Path(temp), "linux")
            (data / "ipfs-repo").mkdir()
            with self.assertRaisesRegex(AssertionError, "starts without the package"):
                self.execute(home, data, evidence, carrier=True)
            self.holder.assert_not_called()
            self.gateway.assert_not_called()

    def test_receipt_hash_and_disk_refuse_launch(self):
        for failure in ("receipt", "tree", "dirty", "parity", "built", "components", "hash", "provider", "disk"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temp:
                home, data, evidence, receipt = self.fixture(Path(temp), "macos")
                if failure == "receipt":
                    receipt["source"]["commit"] = "d" * 40
                elif failure == "tree":
                    receipt["source"]["tree"] = "e" * 40
                elif failure == "dirty":
                    receipt["source"]["clean"] = False
                elif failure == "parity":
                    receipt["runtime"]["parity"] = False
                elif failure == "built":
                    receipt["runtime"]["built_sha256"] = "sha256:" + "e" * 64
                elif failure == "components":
                    (data / "components.json").write_text("{}")
                elif failure == "hash":
                    (data / "bin/elastos").write_bytes(b"changed fixture Runtime")
                elif failure == "provider":
                    (data / "bin/model-provider").unlink()
                (data / "receipts/source-home-installation.json").write_text(json.dumps(receipt))
                expected = (RuntimeError if failure == "disk" else
                            FileNotFoundError if failure == "provider" else AssertionError)
                with self.assertRaises(expected):
                    self.execute(home, data, evidence, available=(1 if failure == "disk" else 20) * 1024 ** 3)
                self.gateway.assert_not_called()
                self.node.assert_not_called()
                self.stop.assert_not_called()

    def test_fresh_fixture_preserves_artifacts_and_refuses_existing_home(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        with tempfile.TemporaryDirectory() as temp:
            home, data, _, _ = self.fixture(Path(temp), "macos")
            (data / "passkeys.json").write_text("private fixture identity")
            (data / "model-provider").mkdir()
            (data / "model-provider/journal").write_text("old run")
            target_home = Path(temp) / "fresh"
            target = journey["fresh_fixture"](home, data, target_home)
            self.assertEqual((target / "bin/model-provider").read_bytes(), (data / "bin/model-provider").read_bytes())
            self.assertEqual((target / "receipts/source-home-installation.json").read_bytes(),
                             (data / "receipts/source-home-installation.json").read_bytes())
            self.assertFalse((target / "passkeys.json").exists())
            self.assertFalse((target / "model-provider").exists())
            _, _, info, source_bundle = journey["engine_paths"](data)
            self.assertEqual((target / "bin/llama-server").resolve(),
                             (target / info["install_path"] / info["binary_path"]).resolve())
            absent = journey["fresh_fixture"](home, data, Path(temp) / "absent", include_engine=False)
            for path in (absent / "bin/llama-server", absent / info["install_path"], absent / "capsules/llama-server"):
                self.assertFalse(path.exists() or path.is_symlink())
            self.assertTrue(source_bundle.exists())
            with self.assertRaises(FileExistsError):
                journey["fresh_fixture"](home, data, target_home)
            with mock.patch.dict(journey["fresh_fixture"].__globals__, {"disk_observation": lambda _: {
                    "capacity_bytes": 100 * 1024 ** 3, "available_bytes": 1024 ** 3}}):
                low_disk_home = Path(temp) / "low-disk"
                with self.assertRaisesRegex(RuntimeError, "2 GiB free disk reserve"):
                    journey["fresh_fixture"](home, data, low_disk_home)
                self.assertFalse(low_disk_home.exists())

    def test_engine_absence_receipt_preserves_post_ui_filesystem_failure(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        for appeared in (None, "alias", "bundle", "capsule", "ui_missing", "reason", "refusal", "engine_process"):
            with self.subTest(appeared=appeared), tempfile.TemporaryDirectory() as temp:
                home, data, evidence, _ = self.fixture(Path(temp), "linux")
                target_home = Path(temp) / "absent"
                target = journey["fresh_fixture"](home, data, target_home, include_engine=False)
                _, _, _, bundle = journey["engine_paths"](target)
                (target_home / "model-inputs/inputs").mkdir(parents=True)
                child = mock.Mock(pid=12345)
                child.poll.return_value = None
                response = mock.MagicMock()
                response.__enter__.return_value.status = 200

                def node_journey(command, **_):
                    if command[1] == "scripts/ci-model-package.mjs":
                        (evidence / "package.json").write_text(json.dumps({"cid": self.PACKAGE_CID}))
                        return
                    self.assertEqual(command[-1], "--home-only")
                    if appeared == "ui_missing":
                        raise subprocess.CalledProcessError(1, command)
                    (evidence / "home-journey.json").write_text(json.dumps({
                        "dispatch_unavailable_reason": "unknown_reason" if appeared == "reason" else "source_engine_required", "results": {
                        "home_screenshots": "passed", "engine_absent_home": "passed",
                        "engine_absent_refusal": "failed" if appeared == "refusal" else "passed"}}))
                    if appeared == "alias":
                        (target / "bin/llama-server").write_text("engine appeared during UI")
                    elif appeared == "bundle":
                        bundle.mkdir(parents=True)
                    elif appeared == "capsule":
                        (target / "capsules/llama-server").mkdir(parents=True)

                with mock.patch.object(subprocess, "check_output", side_effect=lambda args, **_: (
                        ("c" if args[-1] == "HEAD" else "d") * 40 + "\n")), \
                        mock.patch.object(subprocess, "Popen", return_value=child), \
                        mock.patch.object(subprocess, "run", side_effect=node_journey), \
                        mock.patch("urllib.request.urlopen", return_value=response), \
                        mock.patch.dict(journey["run"].__globals__, {"disk_observation": lambda _: {
                            "capacity_bytes": 100 * 1024 ** 3, "available_bytes": 20 * 1024 ** 3},
                            "process_rows": lambda: {},
                            "cleanup_runtime": lambda *_, **__: {"status": "passed", "before": {"llama_server": 1 if appeared == "engine_process" else 0}, "after": {"llama_server": 0}}}):
                    if appeared:
                        with self.assertRaises((AssertionError, RuntimeError, subprocess.CalledProcessError)):
                            journey["run"](target_home, target, evidence, model=False)
                    else:
                        journey["run"](target_home, target, evidence, model=False)
                record = json.loads((evidence / "installed-journeys.json").read_text())
                self.assertTrue(record["engine_absent"])
                self.assertEqual(record["engine_absent_after_ui"], appeared in (None, "engine_process"))
                self.assertEqual(record["results"]["engine_absent_home"], "passed" if appeared in (None, "engine_process") else "failed")
                self.assertEqual(record["results"]["engine_absent_refusal"], "failed" if appeared else "passed")
                self.assertNotIn("unknown_reason", json.dumps(record))
                self.assertEqual(record["results"]["process_cleanup"], "passed")

    def test_engine_receipt_refuses_changed_bytes_and_build_provenance(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        for fault in ("bytes", "recipe", "duplicate"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                _, data, _, _ = self.fixture(Path(temp), "macos")
                self.assertEqual(journey["engine_receipt"](data)["platform"], "darwin-arm64")
                _, _, info, bundle = journey["engine_paths"](data)
                if fault == "bytes":
                    (bundle / info["binary_path"]).write_bytes(b"changed executable")
                elif fault == "recipe":
                    path = bundle / "PROVENANCE.json"
                    value = json.loads(path.read_text())
                    value["recipe_sha256"] = "f" * 64
                    path.write_text(json.dumps(value))
                    # Even a matching local file record cannot change the pinned recipe.
                    receipt_path = bundle / ".elastos-engine.json"
                    receipt = json.loads(receipt_path.read_text())
                    next(row for row in receipt["entries"] if row["path"] == "PROVENANCE.json")["sha256"] = "sha256:" + journey["digest"](path)
                    receipt_path.write_text(json.dumps(receipt))
                else:
                    path = bundle / ".elastos-engine.json"
                    value = json.loads(path.read_text())
                    value["entries"].append(value["entries"][0])
                    path.write_text(json.dumps(value))
                with self.assertRaises(AssertionError):
                    journey["engine_receipt"](data)

    def test_cleanup_reaps_detached_engine_and_preserves_reused_or_foreign_pids(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        data = Path("/isolated/owned/data")
        row = lambda parent, command, start="original": {"ppid": parent, "command": command, "start": start}
        initial = {10: row(1, str(data / "bin/elastos") + " gateway"),
                   11: row(10, str(data / "bin/model-provider")),
                   12: row(11, str(data / "libexec/llama-server") + " -m model"),
                   13: row(1, "/foreign/engine")}
        detached = {12: row(1, initial[12]["command"]), 13: initial[13]}
        gone = {13: initial[13]}
        child = mock.Mock(pid=10)
        child.poll.return_value = None
        with mock.patch.dict(journey["cleanup_runtime"].__globals__, {
                "process_rows": mock.Mock(side_effect=[initial, initial, detached, detached, gone])}), \
                mock.patch.object(os, "killpg") as groups, mock.patch.object(os, "kill") as kill:
            result = journey["cleanup_runtime"](child, data)
        self.assertEqual(result["status"], "passed")
        groups.assert_called_once_with(10, signal.SIGTERM)
        kill.assert_called_once_with(12, signal.SIGTERM)
        # ps start time has one-second precision; command identity also protects reuse.
        reused = {12: row(1, "/foreign/new-process", start="original")}
        child.poll.return_value = 0
        with mock.patch.dict(journey["cleanup_runtime"].__globals__, {
                "process_rows": mock.Mock(side_effect=[initial, reused])}), \
                mock.patch.object(os, "killpg") as groups, mock.patch.object(os, "kill") as kill:
            self.assertEqual(journey["cleanup_runtime"](child, data)["status"], "passed")
            groups.assert_not_called()
            kill.assert_not_called()
        child.poll.return_value = None
        replacement = {10: row(1, "/foreign/replacement-runtime-group")}
        with mock.patch.dict(journey["cleanup_runtime"].__globals__, {
                "process_rows": mock.Mock(side_effect=[initial, replacement, replacement])}), \
                mock.patch.object(os, "killpg") as groups, mock.patch.object(os, "kill") as kill:
            self.assertEqual(journey["cleanup_runtime"](child, data)["status"], "passed")
            groups.assert_not_called()
            kill.assert_not_called()

    def test_three_runs_use_distinct_fresh_state_and_continue_after_one_failed_run(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        for first_fails in (False, True):
            with self.subTest(first_fails=first_fails), tempfile.TemporaryDirectory() as temp:
                home, data, evidence, _ = self.fixture(Path(temp), "macos")
                calls = []
                def one_run(run_home, run_data, run_evidence):
                    calls.append((run_home, run_data, run_evidence))
                    (run_data / "passkeys.json").write_text("owned run state")
                    if first_fails and len(calls) == 1:
                        raise RuntimeError("fixture reply failed")
                with mock.patch.dict(journey["repeat"].__globals__, {"run": one_run}), \
                        mock.patch.object(subprocess, "run") as prepare:
                    if first_fails:
                        with self.assertRaisesRegex(RuntimeError, "1 installed Mac timing journeys failed"):
                            journey["repeat"](home, data, evidence, 3)
                    else:
                        journey["repeat"](home, data, evidence, 3)
                self.assertEqual(len(calls), 3)
                self.assertEqual(len({row[0] for row in calls}), 3)
                self.assertEqual(len({row[1] for row in calls}), 3)
                self.assertEqual(len({row[2] for row in calls}), 3)
                prepare.assert_not_called()
                self.assertFalse((data / "passkeys.json").exists())

    def test_mac_workflow_runs_three_fresh_journeys_and_uploads_their_receipts(self):
        mac_steps = steps("source-home-macos")
        journey, = [step for step in mac_steps
                    if step.startswith("name: installed Marketplace Get and Assistant reply\n")]
        self.assertEqual(field(journey, "run"), "scripts/ci-installed-journeys.sh home-repeat")
        upload, = [step for step in mac_steps if step.startswith("name: upload installed model journey\n")]
        self.assertIn("source-home-journeys/**/*.json", upload)
        self.assertIn("source-home-journeys/**/*.png", upload)


class ModelRepairJourneyTests(unittest.TestCase):
    def test_linux_workflow_prepares_runs_and_requires_model_repair_journey(self):
        workflow = steps("source-home-linux")
        prepare, = [step for step in workflow if step.startswith("name: prepare signed SmolLM2 fixture\n")]
        home, = [step for step in workflow if step.startswith("name: installed Marketplace Get and Assistant reply\n")]
        repair, = [step for step in workflow if "run: scripts/ci-installed-journeys.sh model-repair\n" in step]
        summary, = [step for step in workflow if step.startswith("name: summarize installed model journey\n")]
        browser, = [step for step in workflow if step.startswith("name: prepare installed Home browser\n")]
        self.assertIn("npm ci --ignore-scripts --prefix elastos/tools/browser-playwright-engine", browser)
        self.assertIn("playwright install chromium --with-deps --only-shell", browser)
        before_browser = workflow[workflow.index(browser) - 1]
        after_browser = workflow[workflow.index(browser) + 1]
        self.assertEqual(field(before_browser, "node-version"), "'24'")
        self.assertEqual(field(after_browser, "node-version"), "'26'")
        self.assertEqual(field(prepare, "CI_MODEL_REPAIR_JOURNEY"), "${{ matrix.os == 'ubuntu-24.04' }}")
        self.assertEqual(field(repair, "if"), "matrix.os == 'ubuntu-24.04'")
        self.assertEqual(field(repair, "CI_MODEL_REPAIR_JOURNEY"), "'true'")
        self.assertEqual(field(repair, "CI_SOURCE_HOME"), field(prepare, "CI_SOURCE_HOME"))
        self.assertEqual(field(summary, "CI_SOURCE_HOME"), field(repair, "CI_SOURCE_HOME"))
        self.assertEqual(field(summary, "CI_MODEL_REPAIR_JOURNEY"), field(prepare, "CI_MODEL_REPAIR_JOURNEY"))
        self.assertEqual(field(summary, "if"), "always()")
        for step in (prepare, repair, summary):
            self.assertNotIn("continue-on-error:", step)
        self.assertLess(workflow.index(browser), workflow.index(prepare))
        self.assertLess(workflow.index(prepare), workflow.index(home))
        self.assertLess(workflow.index(home), workflow.index(repair))
        self.assertLess(workflow.index(repair), workflow.index(summary))
        shell = (WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh").read_text()
        self.assertIn('CI_MODEL_REPAIR_JOURNEY:-false', shell)
        self.assertIn('ci-model-package.mjs repair-inputs "${CI_HOME}/model-inputs/qwen-inputs"', shell)
        self.assertIn('"$EVIDENCE/model-repair" --model-repair', shell)

    def test_model_repair_summary_requires_current_receipt_and_every_journey_result(self):
        names = ("model_stop_get_different", "model_interrupted_get_retry", "model_remaining_fetch")
        core = ("home_screenshots", "model_package_admission", "carrier_get", "installed_runtime_reply",
                "model_timing_observer", "process_cleanup", "disk_reserve")
        failures = (None, "missing", "failed_result", "candidate", "source_tree", "installed_runtime_sha256",
                    "installed_model_provider_sha256", "source_components_sha256", "installed_engine", *names, *core)
        for failure in failures:
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory).resolve()
                platform = "macos" if sys.platform == "darwin" else "linux"
                home, data, _, _ = InstalledJourneyTests().fixture(root, platform)
                evidence = root / "source-home-journeys"
                evidence.mkdir()
                journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
                identities = {"candidate": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                              "source_tree": subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], text=True).strip(),
                              "installed_runtime_sha256": journey["digest"](data / "bin/elastos"),
                              "installed_model_provider_sha256": journey["digest"](data / "bin/model-provider"),
                              "source_components_sha256": journey["digest"](data / "components.json")}
                row = {**identities, "installed_engine": journey["engine_receipt"](data),
                       "elapsed_seconds": 5, "fixture_preparation_seconds": 2,
                       "results": {name: "passed" for name in core}}
                if platform == "macos":
                    timing = InstalledModelTimingTests()
                    timing.setUp()
                    row["model_timing"] = {"durations": timing.timing["run_metrics"](*timing.timing_fixture())}
                for index in range(1, 4 if platform == "macos" else 2):
                    run = evidence / ("run-" + str(index))
                    run.mkdir()
                    (run / "installed-journeys.json").write_text(json.dumps(row))
                absent = evidence / "engine-absent-home"
                absent.mkdir()
                (absent / "installed-journeys.json").write_text(json.dumps({
                    **identities, "engine_absent": True, "dispatch_unavailable_reason": "source_engine_required",
                    "process_cleanup": {"before": {"llama_server": 0}},
                    "results": {name: "passed" for name in ("engine_absent_home", "engine_absent_refusal",
                                                            "home_screenshots", "process_cleanup", "disk_reserve")}}))
                repair = {**row, "results": {name: "passed" for name in (*core, *names)}}
                if failure in identities:
                    repair[failure] = "stale"
                elif failure == "failed_result":
                    repair["results"][names[0]] = "failed"
                elif failure == "installed_engine":
                    repair["installed_engine"] = {}
                elif failure in (*names, *core):
                    repair["results"].pop(failure)
                if failure != "missing":
                    (evidence / "model-repair").mkdir()
                    (evidence / "model-repair/installed-journeys.json").write_text(json.dumps(repair))
                environment = {**os.environ, "CI_SOURCE_HOME": str(home), "RUNNER_TEMP": str(root),
                               "GITHUB_STEP_SUMMARY": str(root / "summary.md"), "CI_MODEL_REPAIR_JOURNEY": "true",
                               "CI_CARRIER_GET": "false", "CI_FINITE_MEMORY": "false", "PYTHONDONTWRITEBYTECODE": "1"}
                result = subprocess.run(["/bin/bash", str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh"), "summary"],
                                        env=environment, capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode == 0, failure is None, result.stderr)
                receipt = json.loads((evidence / "core-summary.json").read_text())
                for name in names:
                    failed_name = names[0] if failure == "failed_result" else failure
                    expected = "failed or not run" if failure and (failed_name not in names or failed_name == name) else "passed"
                    self.assertEqual(receipt["results"][name], expected)
                self.assertIn("interrupted Get survives Home restart", (root / "summary.md").read_text())

    def test_engine_pidfd_binding_rejects_process_reuse_and_closes_descriptor(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        owner = journey["RepairRuntime"].__new__(journey["RepairRuntime"])
        owner.data, owner.child, owner.stop = Path("/fixture/data"), mock.Mock(pid=100), mock.Mock()
        owner.stop.is_set.return_value = False
        owner.engine_fds, owner.record = {}, {}
        identities = [dict(pid=103, uid=os.getuid(), start_ticks=ticks, executable="/fixture/engine", device=1, inode=1)
                      for ticks in (71, 72)]
        rows = {103: {"role": "llama_server", "ppid": 102},
                102: {"role": "guard", "ppid": 101}, 101: {"role": "model_provider", "ppid": 100}}
        with mock.patch.dict(owner.engine_running.__globals__, {"process_rows": lambda: [],
                "owned_processes": lambda *_: rows, "linux_process_identity": mock.Mock(side_effect=identities),
                "pidfd_exited": lambda _: False}), \
                mock.patch.object(os, "pidfd_open", return_value=77, create=True), \
                mock.patch.object(os, "close") as closed:
            with self.assertRaisesRegex(RuntimeError, "changed during pidfd binding"):
                owner.engine_running()
        closed.assert_called_once_with(77)
        self.assertEqual(owner.engine_fds, {})

    def test_stopped_owner_cannot_launch_a_runtime_after_cleanup(self):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        owner = journey["RepairRuntime"].__new__(journey["RepairRuntime"])
        owner.stop, owner.launch_lock = journey["threading"].Event(), journey["threading"].Lock()
        owner.stop.set()
        with mock.patch.object(subprocess, "Popen") as launched:
            with self.assertRaisesRegex(RuntimeError, "owner is stopping"):
                owner.launch()
        launched.assert_not_called()

    def fixture(self, root):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        owner = journey["RepairRuntime"].__new__(journey["RepairRuntime"])
        owner.home, owner.data = root / "home", root / "home/data"
        operation = "a" * 64
        admitted = owner.data / ("model-preparation/admitted-" + operation)
        admitted.mkdir(parents=True)
        weights = b"GGUF" + b"pinned-fixture" * 20000
        (admitted / "weights.gguf").write_bytes(weights)
        whole = 2 * 65536
        owner.kept = {"weights_bytes": whole, "weights_prefix_sha256": hashlib.sha256(weights[:whole]).hexdigest(),
                      "interrupted_process_cleanup": {"status": "passed"}}
        (owner.data / "model-preparation/state.json").write_text(json.dumps({"records": [{
            "operation_id": operation, "admission_id": operation, "state": "admitted"}]}))
        log = owner.home / "journey-runtime.private.log"
        log.write_text(f"timestamp target: private model preparation resumed operation_id={operation} completed_bytes={whole + 100}\n"
                       f"timestamp target: private model preparation remaining weights read operation_id={operation} completed_bytes={whole + 100} weights_offset={whole}\n")
        return owner, {"model_repair": {"retry_operation_id": operation}}, hashlib.sha256(weights).hexdigest(), log

    def test_retry_receipt_binds_retained_prefix_and_first_actual_remaining_read(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, ui, digest, log = self.fixture(Path(temp))
            log.write_text("\x1b[32m" + log.read_text() + "\x1b[0m")
            result = owner.retry_receipt(ui, digest)
            self.assertEqual(result["first_remaining_weights_offset"], owner.kept["weights_bytes"])
            self.assertGreater(result["remaining_weights_bytes"], 0)
            self.assertTrue(result["retained_prefix_matches_admitted"])

    def test_retry_receipt_refuses_missing_duplicate_wrong_binding_and_zero_offset(self):
        for fault in ("missing", "duplicate", "foreign", "zero", "count", "field", "prefix", "final"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, ui, digest, log = self.fixture(Path(temp))
                text = log.read_text()
                if fault == "missing": text = text.splitlines()[0] + "\n"
                elif fault == "duplicate": text += text
                elif fault == "foreign": text = text.replace("operation_id=" + "a" * 64, "operation_id=" + "b" * 64)
                elif fault == "zero": text = text.replace("weights_offset=131072", "weights_offset=0")
                elif fault == "count": text = text.replace("completed_bytes=131172", "completed_bytes=0", 1)
                elif fault == "field": text = text.replace("weights_offset=131072", "weights_offset=unknown")
                elif fault == "prefix": owner.kept["weights_prefix_sha256"] = "0" * 64
                elif fault == "final": digest = "0" * 64
                log.write_text(text)
                with self.assertRaises(RuntimeError): owner.retry_receipt(ui, digest)


class FiniteInstalledJourneyTests(unittest.TestCase):
    """Exercise the actual finite owner with explicit kernel/service observations."""
    def setUp(self):
        self.journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        self.globals = self.journey["FiniteRuntime"].observe_once.__globals__

    def proc_process(self, proc, pid=100, ticks=71, state="S", parent=99, executable=None):
        directory = proc / str(pid)
        directory.mkdir(parents=True, exist_ok=True)
        fields = [state, str(parent), str(pid)] + ["0"] * 17
        fields[19] = str(ticks)
        (directory / "stat").write_text(f"{pid} (fixture with spaces) " + " ".join(fields))
        (directory / "status").write_text("Uid:\t1001\t1001\t1001\t1001\n")
        link = directory / "exe"
        if link.is_symlink():
            link.unlink()
        if executable is not None:
            link.symlink_to(executable)
        return directory

    def finite_owner(self, root):
        home, data, _, _ = InstalledJourneyTests().fixture(root, "linux")
        owner = self.journey["FiniteRuntime"].__new__(self.journey["FiniteRuntime"])
        owner.home, owner.data, owner.gib, owner.budget = home, data, 4, 4 * 1024 ** 3
        owner.unit = "elastos-ci-model-fixture.service"
        owner.control = home / "finite-control"
        owner.control.mkdir(mode=0o700)
        owner.binding = {"InvocationID": "a" * 32, "ControlGroup": "/system.slice/" + owner.unit,
                         "MainPID": "99", "ActiveState": "active"}
        owner.group = Path("/sys/fs/cgroup") / owner.binding["ControlGroup"].lstrip("/")
        owner.pid = 100
        owner.known, owner.fds, owner.metric_fds, owner.actions = {}, {}, {}, []
        owner.outside, owner.last_action = [900], 0
        owner.engine, owner.engine_members = None, set()
        owner.stop = mock.Mock()
        owner.stop.is_set.return_value = False
        owner.thread, owner.error, owner.failure, owner.launch_requested = None, None, None, True
        owner.baseline_events = {"oom": 0, "oom_kill": 0}
        owner.show = mock.Mock(return_value=dict(owner.binding))
        owner.command = mock.Mock()
        values = {"memory.max": str(owner.budget), "memory.swap.max": "0", "pids.max": "512",
                  "memory.current": "100", "memory.peak": "200", "memory.events": "oom 0\noom_kill 0",
                  "cgroup.events": "populated 0"}
        owner.metric = mock.Mock(side_effect=lambda name: values[name])
        return owner, values

    def process_rows(self, owner):
        _, _, info, bundle = self.journey["engine_paths"](owner.data)
        paths = {99: str(Path(sys.executable).resolve()), 100: str(owner.data / "bin/elastos"),
                 101: str(owner.data / "bin/model-provider"), 102: str(owner.data / "bin/model-provider"),
                 103: str((bundle / info["binary_path"]).resolve())}
        return {pid: {"pid": pid, "uid": 1001, "start_ticks": pid + 700, "parent": parent,
                      "group": 103 if pid == 103 else pid, "state": "S", "executable": paths[pid],
                      "device": Path(paths[pid]).stat().st_dev, "inode": Path(paths[pid]).stat().st_ino,
                      "exe_sha256": self.journey["digest"](Path(paths[pid])),
                      "role": {99: "helper", 100: "runtime", 101: "model_provider", 102: "guard", 103: "llama_server"}[pid]}
                for pid, parent in ((99, 900), (100, 99), (101, 100), (102, 101), (103, 102))}

    def observations(self, owner, rows):
        return mock.patch.dict(self.globals, {
            "bounded_text": lambda path, *_: ("--internal-local-llama-guard\0" if Path(path).parent.name == "102" else "fixture\0")
                            if Path(path).name == "cmdline" else " ".join(map(str, rows)),
            "linux_process_identity": lambda pid, **_: dict(rows[pid]),
            "cgroup_membership": lambda pid, *_: "/outside" if pid in owner.outside else owner.binding["ControlGroup"],
            "pidfd_exited": lambda _: False,
        })

    def test_finite_readers_bound_control_data_and_preserve_unsafe_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root / "control.json"
            self.journey["private_json"](path, {"id": 1})
            self.assertEqual(self.journey["private_json"](path), {"id": 1})
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            path.chmod(0o644)
            with self.assertRaisesRegex(RuntimeError, "ownership"):
                self.journey["private_json"](path)
            path.chmod(0o600)
            foreign = list(path.stat())
            foreign[4] = os.getuid() + 1
            with mock.patch.object(os, "fstat", return_value=os.stat_result(foreign)), \
                    self.assertRaisesRegex(RuntimeError, "ownership"):
                self.journey["private_json"](path)
            hardlink = root / "hardlink.json"
            os.link(path, hardlink)
            with self.assertRaisesRegex(RuntimeError, "ownership"):
                self.journey["private_json"](path)
            hardlink.unlink()
            link = root / "link.json"
            link.symlink_to(path)
            with self.assertRaises(OSError):
                self.journey["private_json"](link)
            path.write_bytes(b"x" * 65537)
            with self.assertRaisesRegex(RuntimeError, "bound"):
                self.journey["private_json"](path)
            with self.assertRaisesRegex(RuntimeError, "bound"):
                self.journey["bounded_text"](path, 65536)
            with self.assertRaisesRegex(RuntimeError, "bound"):
                self.journey["private_json"](root / "new.json", {"private": "x" * 65537})
            self.assertFalse((root / "new.json").exists())
            self.assertTrue(link.is_symlink())

    def test_finite_z_leader_requires_retained_birth_and_whole_process_pidfd_exit(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            executable = root / "engine"
            executable.write_bytes(b"fixture; never executed")
            proc = root / "proc"
            self.proc_process(proc, executable=executable)
            identity = self.journey["linux_process_identity"](100, proc)
            self.proc_process(proc, state="Z")
            retained = self.journey["linux_process_identity"](100, proc, retained=identity)
            self.assertEqual(retained["state"], "Z")
            self.assertTrue(self.journey["same_process"](identity, retained))
            with self.assertRaises(FileNotFoundError):
                self.journey["linux_process_identity"](100, proc)
            self.proc_process(proc, ticks=72, state="Z")
            with self.assertRaises(FileNotFoundError):
                self.journey["linux_process_identity"](100, proc, retained=identity)
            selector = mock.Mock(POLLIN=1, POLLERR=8, POLLHUP=16, POLLNVAL=32)
            for events, exited in (([], False), ([(45, selector.POLLIN)], True),
                                   ([(45, selector.POLLIN | selector.POLLHUP)], True)):
                poll = mock.Mock()
                poll.poll.return_value = events
                selector.poll.return_value = poll
                with mock.patch.dict(self.globals, {"select": selector}):
                    self.assertEqual(self.journey["pidfd_exited"](45), exited)
                poll.register.assert_called_once_with(45, selector.POLLIN)
                poll.poll.assert_called_once_with(0)
            for events in ([(45, selector.POLLHUP)], [(45, selector.POLLERR)], [(45, selector.POLLNVAL)],
                           [(45, 2)], [(45, selector.POLLIN | selector.POLLERR)],
                           [(45, selector.POLLIN | selector.POLLHUP | selector.POLLERR)],
                           [(46, selector.POLLIN)], [(45, selector.POLLIN), (45, selector.POLLIN)]):
                poll = mock.Mock()
                poll.poll.return_value = events
                selector.poll.return_value = poll
                with mock.patch.dict(self.globals, {"select": selector}), \
                        self.assertRaisesRegex(RuntimeError, "exit is unavailable"):
                    self.journey["pidfd_exited"](45)

    def test_finite_preflight_requires_global_v2_visibility_and_explicit_platform(self):
        for fault in (None, "platform", "hidden_root", "subtree_type", "root_max", "root_current", "v1",
                      "controller", "overmount", "hidden_parent", "ancestor_overmount", "membership",
                      "unreadable_type", "mount_changed", "membership_changed"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = Path(temp) / "cgroup"
                proc = Path(temp) / "proc"
                root.mkdir()
                (proc / "self").mkdir(parents=True)
                mount_root = "/private-subtree" if fault == "hidden_root" else "/"
                kind = "cgroup" if fault == "v1" else "cgroup2"
                mount = "1 0 0:0 / / rw - rootfs rootfs rw\n"
                mount += f"2 {900 if fault == 'hidden_parent' else 1} 0:1 {mount_root} {root} rw - {kind} cgroup rw\n"
                if fault == "overmount":
                    mount += f"3 2 0:2 / {root}/covered rw - tmpfs tmpfs rw\n"
                if fault == "ancestor_overmount":
                    mount += f"3 1 0:2 / {root.parent} rw - tmpfs tmpfs rw\n"
                (proc / "self/mountinfo").write_text(mount)
                (proc / "self/cgroup").write_text("0::/outside\n" if fault != "membership" else "1:memory:/hidden\n")
                (root / "cgroup.controllers").write_text("cpu pids" if fault == "controller" else "cpu memory pids")
                if fault == "subtree_type":
                    (root / "cgroup.type").write_text("domain\n")
                if fault in ("root_max", "root_current"):
                    (root / ("memory.max" if fault == "root_max" else "memory.current")).write_text("max\n")
                reads = {}
                reader = self.journey["bounded_text"]

                def observation(path, limit):
                    path = Path(path)
                    reads[path] = reads.get(path, 0) + 1
                    if fault == "unreadable_type" and path.name == "cgroup.type":
                        raise PermissionError("fixture observation refused")
                    value = reader(path, limit)
                    if fault == "mount_changed" and path.name == "mountinfo" and reads[path] > 1:
                        return value + "\n"
                    if fault == "membership_changed" and path.name == "cgroup" and reads[path] > 1:
                        return "0::/changed\n"
                    return value

                with mock.patch.dict(self.globals, {"bounded_text": observation}), \
                        mock.patch.object(os, "pidfd_open", create=True), \
                        mock.patch.object(signal, "pidfd_send_signal", create=True):
                    if fault:
                        with self.assertRaises((RuntimeError, OSError)):
                            self.journey["finite_memory_preflight"]("darwin" if fault == "platform" else "linux", proc, root)
                    else:
                        self.assertEqual(self.journey["finite_memory_preflight"]("linux", proc, root), "/outside")

    def test_finite_census_binds_pidfds_and_rejects_reuse_escaped_children_and_oom(self):
        for fault in (None, "birth", "uid", "escaped", "observer_inside", "oom", "peak", "limit_changed", "swap_changed",
                      "duplicate", "census_bound", "omitted_live", "omitted_exited", "nested", "unknown_executable"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, values = self.finite_owner(Path(temp))
                rows = self.process_rows(owner)
                with self.observations(owner, rows), mock.patch.object(os, "getuid", return_value=1001), \
                        mock.patch.object(os, "pidfd_open", side_effect=lambda pid, flags: pid + 1000, create=True):
                    self.assertEqual(set(owner.observe_once()), set(rows))
                    self.assertEqual(set(owner.fds), set(rows))
                    self.assertEqual(owner.engines(owner.known), [103])
                    if fault is None:
                        continue
                    if fault == "birth":
                        rows[103]["start_ticks"] += 1
                    elif fault == "uid":
                        rows[103]["uid"] = 9999
                    elif fault == "escaped":
                        self.globals["cgroup_membership"] = lambda *_: "/foreign"
                    elif fault == "nested":
                        self.globals["cgroup_membership"] = lambda pid, *_: ("/outside" if pid in owner.outside else
                                                                           owner.binding["ControlGroup"] + "/nested")
                    elif fault == "unknown_executable":
                        executable = Path(temp) / "foreign-child"
                        executable.write_bytes(b"unowned fixture")
                        rows[103].update(executable=str(executable), device=executable.stat().st_dev, inode=executable.stat().st_ino)
                    elif fault == "observer_inside":
                        self.globals["cgroup_membership"] = lambda *_: owner.binding["ControlGroup"]
                    elif fault == "oom":
                        values["memory.events"] = "oom 1\noom_kill 0"
                    elif fault == "peak":
                        values["memory.peak"] = str(owner.budget + 1)
                    elif fault == "limit_changed":
                        values["memory.max"] = "max"
                    elif fault == "swap_changed":
                        values["memory.swap.max"] = "max"
                    elif fault == "duplicate":
                        self.globals["bounded_text"] = lambda *_: "100 100"
                    elif fault == "census_bound":
                        self.globals["bounded_text"] = lambda *_: " ".join(map(str, range(513)))
                    else:
                        rows.pop(103)
                        if fault == "omitted_exited":
                            self.globals["pidfd_exited"] = lambda fd: fd == owner.fds[103]
                            self.assertNotIn(103, owner.observe_once())
                            self.assertIn(103, owner.fds)
                            continue
                    with self.assertRaises(RuntimeError):
                        owner.observe_once()

    def test_finite_census_accepts_same_birth_exec_and_never_rebinds_an_exited_pidfd(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, _ = self.finite_owner(Path(temp))
            rows = self.process_rows(owner)
            engine = dict(rows[103])
            rows[103].update({key: rows[102][key] for key in ("executable", "device", "inode")})
            with self.observations(owner, rows), mock.patch.object(os, "getuid", return_value=1001), \
                    mock.patch.object(os, "pidfd_open", side_effect=lambda pid, flags: pid + 1000, create=True) as opened:
                owner.observe_once()
                rows[103] = engine
                self.assertEqual(owner.engines(owner.observe_once()), [103])
                self.assertEqual(opened.call_count, len(rows))
                self.globals["pidfd_exited"] = lambda fd: fd == owner.fds[103]
                self.assertNotIn(103, owner.observe_once())
                self.assertEqual(opened.call_count, len(rows))

    def test_finite_pidfd_binding_closes_on_identity_race_or_unreadable_child(self):
        for fault in ("reused", "disappeared", "denied"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                row = self.process_rows(owner)[100]
                second = (dict(row, start_ticks=row["start_ticks"] + 1) if fault == "reused" else
                          FileNotFoundError("fixture child exited") if fault == "disappeared" else
                          PermissionError("fixture identity unavailable"))
                with self.observations(owner, {100: row}), \
                        mock.patch.dict(self.globals, {"linux_process_identity": mock.Mock(side_effect=[row, second])}), \
                        mock.patch.object(os, "getuid", return_value=1001), \
                        mock.patch.object(os, "pidfd_open", return_value=1100, create=True), \
                        mock.patch.object(os, "close") as closed:
                    with self.assertRaises((RuntimeError, OSError)):
                        owner.observe_once()
                closed.assert_called_once_with(1100)
                self.assertEqual(owner.fds, {})
                self.assertEqual(owner.known, {})

    def test_finite_helper_rejects_changed_owner_or_runtime_and_closes_failed_launch_pidfd(self):
        for fault in ("owner", "runtime", "launch"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                runtime = root / "elastos"
                runtime.write_bytes(b"exact installed Runtime fixture")
                identity = {"pid": 900, "uid": 1001, "start_ticks": 71, "executable": "/fixture/python",
                            "device": 1, "inode": 2}
                payload = {"owner": identity, "runtime": str(runtime), "runtime_sha256": self.journey["digest"](runtime),
                           "environment": {}, "address": "127.0.0.1:1", "log": str(root / "private.log")}
                if fault == "runtime":
                    payload["runtime_sha256"] = "f" * 64
                self.journey["private_json"](root / "launch.json", payload)
                observed = dict(identity, start_ticks=72) if fault == "owner" else identity
                with mock.patch.dict(self.globals, {"linux_process_identity": lambda *_: observed,
                                                    "pidfd_exited": lambda _: False}), \
                        mock.patch.object(os, "pidfd_open", return_value=77, create=True) as opened, \
                        mock.patch.object(os, "close") as closed, \
                        mock.patch.object(signal, "signal"), \
                        mock.patch.object(subprocess, "Popen", side_effect=RuntimeError("fixture launch failed")) as launch:
                    with self.assertRaises(RuntimeError):
                        self.journey["runtime_unit_helper"](root)
                if fault == "owner":
                    opened.assert_not_called()
                    closed.assert_not_called()
                else:
                    opened.assert_called_once_with(900, 0)
                    closed.assert_called_once_with(77)
                self.assertEqual(launch.call_count, int(fault == "launch"))
                self.assertFalse((root / "runtime.json").exists())
                self.assertFalse((root / "runtime-stopped.json").exists())

    def test_finite_owner_refuses_existing_control_root_and_invalid_budget_before_launch(self):
        for fault in ("directory", "symlink", "root_uid", "budget"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                home, data, _, _ = InstalledJourneyTests().fixture(Path(temp), "linux")
                control = home / "finite-control"
                if fault == "directory":
                    control.mkdir(mode=0o700)
                    (control / "retained").write_text("owned earlier evidence")
                elif fault == "symlink":
                    control.symlink_to(home / "missing")
                with mock.patch.dict(self.globals, {"finite_memory_preflight": lambda *_: "/outside"}), \
                        mock.patch.object(os, "getuid", return_value=0 if fault == "root_uid" else 1001), \
                        mock.patch.object(subprocess, "run") as command:
                    with self.assertRaises((RuntimeError, FileExistsError)):
                        self.journey["FiniteRuntime"](home, data, "127.0.0.1:1", {}, 2 if fault == "budget" else 4,
                                                      platform="linux")
                command.assert_not_called()
                if fault == "directory":
                    self.assertEqual((control / "retained").read_text(), "owned earlier evidence")
                elif fault == "symlink":
                    self.assertTrue(control.is_symlink())
                else:
                    self.assertFalse(control.exists())

    def test_finite_helper_reaps_its_actual_child_on_owner_exit_and_escalates_wait_timeout(self):
        for wait_timeout in (False, True):
            with self.subTest(wait_timeout=wait_timeout), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                runtime = owner.data / "bin/elastos"
                identity = {"pid": 900, "uid": 1001, "start_ticks": 71, "executable": "/fixture/python",
                            "device": 1, "inode": 2}
                runtime_identity = self.process_rows(owner)[100]
                environment = {"HOME": str(owner.home)}
                self.journey["private_json"](owner.control / "launch.json", {
                    "owner": identity, "runtime": str(runtime), "runtime_sha256": self.journey["digest"](runtime),
                    "environment": environment, "address": "127.0.0.1:1", "log": str(owner.home / "private.log")})
                child = mock.Mock(pid=100)
                child.poll.return_value = None
                child.wait.side_effect = [subprocess.TimeoutExpired("fixture Runtime", 15), 0] if wait_timeout else [0]
                with mock.patch.dict(self.globals, {
                        "linux_process_identity": mock.Mock(side_effect=[identity, identity, runtime_identity]),
                        "pidfd_exited": mock.Mock(side_effect=[False, True, True])}), \
                        mock.patch.object(os, "pidfd_open", return_value=77, create=True) as opened, \
                        mock.patch.object(os, "close") as closed, mock.patch.object(signal, "signal"), \
                        mock.patch.object(subprocess, "Popen", return_value=child) as launch:
                    self.journey["runtime_unit_helper"](owner.control)
                opened.assert_called_once_with(900, 0)
                closed.assert_called_once_with(77)
                self.assertEqual(launch.call_args.args[0], [str(runtime), "gateway", "--addr", "127.0.0.1:1"])
                self.assertEqual(launch.call_args.kwargs["env"], environment)
                self.assertTrue(launch.call_args.kwargs["start_new_session"])
                child.terminate.assert_called_once()
                self.assertEqual(child.kill.call_count, int(wait_timeout))
                self.assertEqual(child.wait.call_args_list, [mock.call(timeout=15)] + ([mock.call(timeout=5)] if wait_timeout else []))
                self.assertEqual(self.journey["private_json"](owner.control / "runtime.json"), runtime_identity)
                self.assertEqual(self.journey["private_json"](owner.control / "runtime-stopped.json"), {"reaped": True})

    def test_finite_engine_crash_uses_only_the_selected_pidfd_and_checks_group_exit(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, _ = self.finite_owner(Path(temp))
            rows = self.process_rows(owner)
            rows[104] = dict(rows[102], pid=104, start_ticks=804, parent=103, group=103)
            owner.known = rows
            owner.fds = {pid: pid + 1000 for pid in rows}
            owner.observe_once = mock.Mock(return_value=rows)
            with mock.patch.dict(self.globals, {"pidfd_exited": lambda _: False}), \
                    mock.patch.object(signal, "pidfd_send_signal", create=True) as kill, \
                    mock.patch.object(os, "kill") as numeric_kill:
                owner.action({"id": 1, "action": "engine_running"})
                owner.action({"id": 2, "action": "crash_engine"})
                kill.assert_called_once_with(owner.fds[103], signal.SIGKILL)
                numeric_kill.assert_not_called()
                # A vanished executable/leader is not proof that its whole group exited.
                owner.observe_once.return_value = {pid: row for pid, row in rows.items() if pid != 103}
                clock = mock.Mock()
                clock.monotonic.side_effect = [0, 0, 0, 20]
                with mock.patch.dict(self.globals, {"time": clock}), \
                        self.assertRaisesRegex(RuntimeError, "group exit is unconfirmed"):
                    owner.action({"id": 3, "action": "engine_stopped"})
                self.assertEqual(owner.last_action, 2)
            with mock.patch.dict(self.globals, {"pidfd_exited": lambda _: True}):
                owner.action({"id": 3, "action": "engine_stopped"})
            self.assertEqual([row["action"] for row in owner.actions], ["engine_running", "crash_engine", "engine_stopped"])

    def test_finite_engine_group_exit_waits_for_every_retained_pidfd(self):
        for action in ("engine_stopped", "idle_stopped"):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                rows = self.process_rows(owner)
                rows[102]["group"] = rows[103]["group"]
                owner.known, owner.fds = rows, {pid: pid + 1000 for pid in rows}
                owner.observe_once = mock.Mock(return_value=rows)
                owner.action({"id": 1, "action": "engine_running"})
                self.assertEqual(owner.engine_members, {102, 103})
                now, samples = [0], []
                clock = mock.Mock()
                clock.monotonic.side_effect = lambda: now[0]

                def observe():
                    samples.append(len(samples) + 1)
                    if action == "idle_stopped":
                        now[0] = max(now[0], 51)
                    return {pid: row for pid, row in rows.items() if pid != 103}

                def wait(seconds):
                    self.assertEqual(owner.last_action, 1)
                    self.assertEqual(len(owner.actions), 1)
                    self.assertEqual(self.journey["private_json"](owner.control / "response.json")["id"], 1)
                    now[0] += seconds

                owner.observe_once.side_effect = observe
                owner.stop.wait.side_effect = wait
                with mock.patch.dict(self.globals, {"time": clock,
                        "pidfd_exited": lambda fd: fd != owner.fds[102] or len(samples) >= 2}):
                    owner.action({"id": 2, "action": action})
                self.assertEqual(samples, [1, 2])
                owner.stop.wait.assert_called_once_with(.1)
                self.assertEqual(owner.engine_members, {102, 103})
                self.assertEqual(owner.last_action, 2)
                self.assertEqual(owner.actions[-1]["action"], action)
                self.assertEqual(self.journey["private_json"](owner.control / "response.json")["status"], "passed")

    def test_finite_idle_checks_engine_exit_before_waiting_for_its_guard(self):
        for engine_exited_early in (True, False):
            with self.subTest(engine_exited_early=engine_exited_early), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                rows = self.process_rows(owner)
                rows[102]["group"] = rows[103]["group"]
                owner.known, owner.fds = rows, {pid: pid + 1000 for pid in rows}
                owner.observe_once = mock.Mock(return_value=rows)
                owner.action({"id": 1, "action": "engine_running"})
                now, samples, clock = [0], [], mock.Mock()
                clock.monotonic.side_effect = lambda: now[0]

                def observe():
                    samples.append(len(samples) + 1)
                    now[0] = 49 if len(samples) == 1 else 51
                    # Early engine exit leaves the guard; parent-chain absence
                    # leaves a live engine after the guard itself has exited.
                    removed = 103 if engine_exited_early else 102
                    return {pid: row for pid, row in rows.items() if pid != removed}

                def exited(fd):
                    if len(samples) >= 2:
                        return True
                    return fd == owner.fds[103 if engine_exited_early else 102]

                owner.observe_once.side_effect = observe
                with mock.patch.dict(self.globals, {"time": clock, "pidfd_exited": exited}):
                    if engine_exited_early:
                        with self.assertRaisesRegex(RuntimeError, "idle engine closed early"):
                            owner.action({"id": 2, "action": "idle_stopped"})
                        owner.stop.wait.assert_not_called()
                        self.assertEqual(owner.last_action, 1)
                    else:
                        owner.action({"id": 2, "action": "idle_stopped"})
                        owner.stop.wait.assert_called_once_with(.1)
                        self.assertEqual(samples, [1, 2])
                        self.assertEqual(owner.last_action, 2)

    def test_finite_engine_group_exit_uses_the_original_deadline(self):
        for action, limit in (("engine_stopped", 20), ("idle_stopped", 95)):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                owner.engine, owner.engine_members, owner.fds = 103, {102, 103}, {102: 1102, 103: 1103}
                owner.observe_once = mock.Mock(return_value={})
                owner.engines = mock.Mock(return_value=[])
                now, clock = [0], mock.Mock()
                clock.monotonic.side_effect = lambda: now[0]
                owner.stop.wait.side_effect = lambda _: now.__setitem__(0, limit)
                with mock.patch.dict(self.globals, {"time": clock, "pidfd_exited": lambda _: False}), \
                        self.assertRaisesRegex(RuntimeError, "group exit is unconfirmed"):
                    owner.action({"id": 1, "action": action})
                owner.stop.wait.assert_called_once_with(.1)
                self.assertEqual(now[0], limit)
                self.assertEqual(owner.last_action, 0)
                self.assertEqual(owner.actions, [])
                self.assertFalse((owner.control / "response.json").exists())

    def test_finite_engine_group_wait_preserves_census_failure_and_interruption(self):
        for fault in ("census", "interrupted"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                owner.engine, owner.engine_members, owner.fds = 103, {103}, {103: 1103}
                owner.engines = mock.Mock(return_value=[])
                owner.observe_once = mock.Mock(side_effect=[{}, RuntimeError("finite journey child escaped its finite unit")])
                clock = mock.Mock()
                clock.monotonic.return_value = 0
                if fault == "interrupted":
                    owner.stop.wait.side_effect = lambda _: setattr(owner.stop.is_set, "return_value", True)
                expected = "child escaped" if fault == "census" else "stage interrupted"
                with mock.patch.dict(self.globals, {"time": clock, "pidfd_exited": lambda _: False}), \
                        self.assertRaisesRegex(RuntimeError, expected):
                    owner.action({"id": 1, "action": "engine_stopped"})
                owner.stop.wait.assert_called_once_with(.1)
                self.assertEqual(owner.observe_once.call_count, 2 if fault == "census" else 1)
                self.assertEqual(owner.actions, [])
                self.assertFalse((owner.control / "response.json").exists())

    def test_finite_engine_group_wait_preserves_missing_engine_and_early_idle_errors(self):
        for fault in ("missing", "empty_members", "missing_handle", "early_idle", "pidfd_error"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                owner.engine = None if fault == "missing" else 103
                owner.engine_members, owner.fds = {103}, {103: 1103}
                if fault == "empty_members":
                    owner.engine_members.clear()
                elif fault == "missing_handle":
                    owner.fds.clear()
                owner.observe_once = mock.Mock(return_value={})
                owner.engines = mock.Mock(return_value=[])
                clock = mock.Mock()
                clock.monotonic.return_value = 0
                exited = mock.Mock(return_value=True)
                if fault == "pidfd_error":
                    exited.side_effect = RuntimeError("finite journey process exit is unavailable")
                expected = ("closed early" if fault == "early_idle" else
                            "process exit is unavailable" if fault == "pidfd_error" else "group exit is unconfirmed")
                with mock.patch.dict(self.globals, {"time": clock, "pidfd_exited": exited}), \
                        self.assertRaisesRegex(RuntimeError, expected):
                    owner.action({"id": 1, "action": "idle_stopped"})
                owner.stop.wait.assert_not_called()
                self.assertEqual(owner.actions, [])

    def test_finite_observer_saves_phase_type_and_only_public_messages(self):
        secret = "Bearer PrivateToken1234567890 /private/operator/identity.key\n" + "x" * 600
        safe = "finite journey engine group exit is unconfirmed"
        detail = mock.Mock()
        detail.__str__ = mock.Mock(side_effect=AssertionError("private exception detail was formatted"))
        private_type = type("PrivateTypeToken1234567890", (Exception,), {"__str__": detail.__str__})
        errors = ((RuntimeError(safe), "RuntimeError", safe),
                  (RuntimeError(secret), "RuntimeError", "observer detail redacted"),
                  (OSError(2, secret), "FileNotFoundError", "observer detail redacted"),
                  (ValueError(secret), "ValueError", "observer detail redacted"),
                  (RuntimeError(detail), "RuntimeError", "observer detail redacted"),
                  (private_type(secret), "redacted", "observer detail redacted"))
        for index, (error, expected_type, expected_message) in enumerate(errors):
            with self.subTest(index=index), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                owner.started, owner.launch_requested = 0, False
                owner.observe_once = mock.Mock(side_effect=error)
                clock = mock.Mock()
                clock.monotonic.return_value = 1
                with mock.patch.dict(self.globals, {"time": clock}):
                    owner.observe()
                self.assertEqual(owner.error, "finite_resource_observation_failed")
                self.assertEqual(owner.failure["phase"], "census")
                self.assertEqual(owner.failure["type"], expected_type)
                self.assertEqual(owner.failure["message"], expected_message)
                receipt = owner.finish()
                encoded = json.dumps(receipt) + self.globals["failure_suffix"](owner.failure)
                self.assertEqual(receipt["observer_failure"], owner.failure)
                self.assertEqual(receipt["status"], "failed")
                self.assertLessEqual(len(owner.failure["message"]), 480)
                self.assertNotIn("\n", owner.failure["message"])
                for private in ("PrivateToken", "PrivateTypeToken", "/private/operator", "identity.key", "x" * 50):
                    self.assertNotIn(private, encoded)
                self.assertEqual(self.journey["private_json"](owner.control / "response.json")["status"], "failed")
                self.assertEqual(self.journey["private_json"](owner.control / "stop.json"), {"stop": True})
        detail.__str__.assert_not_called()

    def test_finite_observer_stage_phase_uses_only_validated_action_text(self):
        for action in ("engine_stopped", "private-action-Token1234567890"):
            with self.subTest(action=action), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                owner.started, owner.launch_requested = 0, False
                owner.observe_once = mock.Mock(return_value={})
                owner.action = mock.Mock(side_effect=RuntimeError("finite journey stage is unsupported"))
                self.journey["private_json"](owner.control / "request.json", {"id": 1, "action": action})
                clock = mock.Mock()
                clock.monotonic.return_value = 1
                with mock.patch.dict(self.globals, {"time": clock}):
                    owner.observe()
                expected = "stage_engine_stopped" if action == "engine_stopped" else "stage_request"
                self.assertEqual(owner.failure["phase"], expected)
                self.assertNotIn("Token123", json.dumps(owner.finish()))

    def test_finite_engine_stage_refuses_wrong_binding_interruption_and_deadline(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, _ = self.finite_owner(Path(temp))
            owner.observe_once = mock.Mock(return_value={})
            owner.engines = mock.Mock(return_value=[])
            for request in ({"id": 2, "action": "engine_running"}, {"id": True, "action": "engine_running"},
                            {"id": 1, "action": "foreign"}, {"id": 1, "action": "engine_running", "pid": 123}):
                with self.subTest(request=request), self.assertRaises(RuntimeError):
                    owner.action(request)
            owner.observe_once.assert_not_called()
            owner.stop.is_set.return_value = True
            with self.assertRaisesRegex(RuntimeError, "interrupted"):
                owner.action({"id": 1, "action": "engine_running"})
            owner.stop.is_set.return_value = False
            clock = mock.Mock()
            clock.monotonic.side_effect = [0, 0, 21]
            with mock.patch.dict(self.globals, {"time": clock}), self.assertRaisesRegex(RuntimeError, "deadline"):
                owner.action({"id": 1, "action": "engine_running"})
            self.assertEqual(owner.actions, [])
            self.assertFalse((owner.control / "response.json").exists())

    def test_finite_start_requires_actual_runtime_and_every_kernel_limit(self):
        for fault in (None, "launcher_pid", "wrong_parent", "unit_max", "swap", "tasks", "tighter_parent",
                      "missing_max", "missing_current", "malformed_current"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, values = self.finite_owner(Path(temp))
                rows = self.process_rows(owner)
                runtime = rows[99] if fault == "launcher_pid" else dict(rows[100])
                if fault == "wrong_parent":
                    runtime["parent"] = 900
                self.journey["private_json"](owner.control / "runtime.json", runtime)
                owner.observe_once = mock.Mock(return_value=rows)
                if fault == "unit_max":
                    values["memory.max"] = "max"
                if fault == "swap":
                    values["memory.swap.max"] = "max"
                if fault == "tasks":
                    values["pids.max"] = "max"

                def ancestor(path, *_):
                    if Path(path).name == "memory.max":
                        if fault == "missing_max":
                            raise FileNotFoundError(path)
                        if Path(path).parent == owner.group:
                            return str(owner.budget)
                        return str(owner.budget - 1) if fault == "tighter_parent" else "max"
                    if fault == "missing_current":
                        raise FileNotFoundError(path)
                    return "bad" if fault == "malformed_current" else "100"

                with mock.patch.dict(self.globals, {"private_json": lambda _: runtime,
                                                    "linux_process_identity": lambda pid, **_: runtime,
                                                    "bounded_text": ancestor}), \
                        mock.patch.object(os, "getuid", return_value=1001), \
                        mock.patch.object(os, "open", side_effect=range(700, 707)), \
                        mock.patch.object(self.globals["threading"], "Thread") as observer:
                    if fault:
                        with self.assertRaises((RuntimeError, FileNotFoundError)):
                            owner.start()
                        observer.assert_not_called()
                        owner.observe_once.assert_not_called()
                    else:
                        self.assertIs(owner.start(), owner)
                        self.assertEqual(owner.pid, 100)
                        owner.observe_once.assert_called_once()
                        observer.return_value.start.assert_called_once()
                self.assertEqual(owner.command.call_args.args[0], "systemd-run")
                self.assertIn("--property=MemoryMax=" + str(owner.budget), owner.command.call_args.args)
                self.assertIn("--property=MemorySwapMax=0", owner.command.call_args.args)

    def test_finite_cleanup_needs_whole_exit_reap_empty_unit_and_finished_observer(self):
        for fault in (None, "live_sibling", "populated", "oom", "peak", "unreaped", "observer", "unit_reused", "unit_replaced_late",
                      "partial_start", "stop_failure", "reap_timeout", "helper_release_timeout", "unknown_child"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, values = self.finite_owner(Path(temp))
                owner.known = self.process_rows(owner)
                owner.fds = {pid: pid + 1000 for pid in owner.known}
                owner.metric_fds = {name: index + 2000 for index, name in enumerate(values)}
                owner.group = Path(temp) / "retained-cgroup"
                owner.group.mkdir()
                owner.observe_once = mock.Mock(return_value={99: owner.known[99]})
                if fault == "unknown_child":
                    owner.observe_once.side_effect = RuntimeError("fixture child executable is outside installed artifacts")
                if fault != "reap_timeout":
                    self.journey["private_json"](owner.control / "runtime-stopped.json", {"reaped": fault != "unreaped"})
                if fault == "populated":
                    values["cgroup.events"] = "populated 1"
                if fault == "oom":
                    values["memory.events"] = "oom 0\noom_kill 1"
                if fault == "peak":
                    values["memory.peak"] = str(owner.budget + 1)
                if fault == "observer":
                    owner.thread = mock.Mock()
                    owner.thread.is_alive.return_value = True
                if fault == "unit_reused":
                    owner.show.return_value["InvocationID"] = "b" * 32
                if fault == "unit_replaced_late":
                    owner.show.side_effect = [dict(owner.binding), dict(owner.binding), dict(owner.binding, InvocationID="b" * 32)]
                if fault == "partial_start":
                    owner.binding = None
                    owner.baseline_events = None
                    owner.metric_fds.pop("memory.max")
                if fault == "stop_failure":
                    owner.command.side_effect = subprocess.TimeoutExpired("systemctl", 40)
                clock = mock.Mock()
                clock.monotonic.side_effect = [0, 26]
                released = False
                write_control = self.journey["private_json"]

                def control(path, value=None):
                    nonlocal released
                    result = write_control(path, value)
                    if Path(path).name == "release.json" and value == {"release": True} and fault != "helper_release_timeout":
                        released = True
                    return result

                patches = {"private_json": control,
                           "pidfd_exited": lambda fd: (fd != owner.fds[99] or released)
                                           and (fault != "live_sibling" or fd != owner.fds[103])}
                if fault in ("reap_timeout", "helper_release_timeout"):
                    if fault == "helper_release_timeout":
                        clock.monotonic.side_effect = [0, 1, 7]
                    patches["time"] = clock
                descriptors = set(owner.fds.values()) | set(owner.metric_fds.values())
                with mock.patch.dict(self.globals, patches), mock.patch.object(os, "close") as closed, \
                        mock.patch.object(signal, "pidfd_send_signal", create=True) as kill:
                    receipt = owner.finish()
                self.assertEqual(receipt["status"], "failed" if fault else "passed", receipt)
                self.assertEqual({call.args[0] for call in closed.call_args_list}, descriptors)
                kill.assert_not_called()
                if fault == "observer":
                    owner.observe_once.assert_not_called()
                    self.assertEqual(receipt["failure_phase"], "observer_stop")
                if fault in ("unit_reused", "unit_replaced_late"):
                    owner.command.assert_not_called()
                else:
                    owner.command.assert_called_once_with("systemctl", "stop", owner.unit)
                if fault is None:
                    self.assertTrue(receipt["whole_process_exit"])
                    self.assertTrue(receipt["runtime_reaped"])
                    self.assertNotEqual(receipt["actual_runtime_pid"], receipt["helper_main_pid"])

    def test_finite_cleanup_reads_kernel_counters_before_helper_exit_prunes_the_unit(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, values = self.finite_owner(Path(temp))
            owner.known = self.process_rows(owner)
            for row in owner.known.values():
                row["uid"] = os.getuid()
            owner.fds = {pid: pid + 1000 for pid in owner.known}
            owner.metric_fds = {name: index + 2000 for index, name in enumerate(values)}
            live = {99, 101}
            self.journey["private_json"](owner.control / "runtime-stopped.json", {"reaped": True})
            captured = []
            write_control = self.journey["private_json"]

            def control(path, value=None):
                result = write_control(path, value)
                if Path(path).name == "release.json" and value == {"release": True}:
                    self.assertIn("memory.peak", captured)
                    self.assertIn("memory.events", captured)
                    self.assertEqual(live, {99})
                    live.remove(99)
                return result

            def metric(name):
                if 99 not in live:
                    raise OSError(19, "fixture cgroup was pruned")
                captured.append(name)
                return "populated 1" if name == "cgroup.events" else values[name]

            def command(*args):
                if args[:2] == ("systemctl", "kill"):
                    live.clear()

            owner.metric.side_effect = metric
            owner.show.side_effect = lambda: dict(owner.binding, ControlGroup=owner.binding["ControlGroup"] if live else "")
            owner.command.side_effect = command
            rows = {pid: row for pid, row in owner.known.items() if pid in live}
            with self.observations(owner, rows), mock.patch.dict(self.globals, {
                    "private_json": control,
                    "bounded_text": lambda path, *_: "fixture\0" if Path(path).name == "cmdline" else " ".join(map(str, live)),
                    "pidfd_exited": lambda fd: fd - 1000 not in live,
                    "linux_process_identity": lambda pid, **_: dict(owner.known[pid])}), \
                    mock.patch.object(os, "close"), \
                    mock.patch.object(signal, "pidfd_send_signal", side_effect=lambda fd, _: live.discard(fd - 1000), create=True) as kill:
                receipt = owner.finish()
            self.assertEqual(receipt["status"], "passed", receipt)
            self.assertTrue(receipt["cgroup_empty"])
            self.assertTrue(receipt["whole_process_exit"])
            self.assertEqual(live, set())
            self.assertEqual(kill.call_args_list, [mock.call(owner.fds[101], signal.SIGTERM)])
            owner.command.assert_called_once_with("systemctl", "stop", owner.unit)

    def test_finite_cleanup_escalates_only_bound_native_pidfds_before_releasing_helper(self):
        for exit_during_signal in (False, True):
            with self.subTest(exit_during_signal=exit_during_signal), tempfile.TemporaryDirectory() as temp:
                owner, values = self.finite_owner(Path(temp))
                owner.known = self.process_rows(owner)
                owner.fds = {pid: pid + 1000 for pid in owner.known}
                owner.metric_fds = {name: index + 2000 for index, name in enumerate(values)}
                self.journey["private_json"](owner.control / "runtime-stopped.json", {"reaped": True})
                live = {99, 101}
                owner.observe_once = mock.Mock(side_effect=lambda **_: {pid: owner.known[pid] for pid in live})
                write_control = self.journey["private_json"]

                def control(path, value=None):
                    result = write_control(path, value)
                    if Path(path).name == "release.json" and value == {"release": True}:
                        self.assertEqual(live, {99})
                        live.remove(99)
                    return result

                def terminate(fd, action):
                    if action == signal.SIGKILL or exit_during_signal:
                        live.remove(fd - 1000)
                    if exit_during_signal:
                        raise ProcessLookupError("fixture native child exited during signal")

                clock = mock.Mock()
                clock.monotonic.side_effect = [0, 1, 2, 3] if exit_during_signal else [0, 1, 17, 18, 19, 20]
                with mock.patch.dict(self.globals, {"private_json": control, "time": clock,
                                                   "pidfd_exited": lambda fd: fd - 1000 not in live}), \
                        mock.patch.object(os, "close"), \
                        mock.patch.object(signal, "pidfd_send_signal", side_effect=terminate, create=True) as kill:
                    receipt = owner.finish()
                self.assertEqual(receipt["status"], "passed", receipt)
                actions = (signal.SIGTERM,) if exit_during_signal else (signal.SIGTERM, signal.SIGKILL)
                self.assertEqual(kill.call_args_list, [mock.call(owner.fds[101], action) for action in actions])
                owner.command.assert_called_once_with("systemctl", "stop", owner.unit)

    def test_finite_helper_holds_reaped_runtime_counters_until_owner_release(self):
        with tempfile.TemporaryDirectory() as temp:
            owner, _ = self.finite_owner(Path(temp))
            runtime = owner.data / "bin/elastos"
            identity = {"pid": 900, "uid": 1001, "start_ticks": 71, "executable": "/fixture/python",
                        "device": 1, "inode": 2}
            self.journey["private_json"](owner.control / "launch.json", {
                "owner": identity, "runtime": str(runtime), "runtime_sha256": self.journey["digest"](runtime),
                "environment": {}, "address": "127.0.0.1:1", "log": str(owner.home / "private.log")})
            self.journey["private_json"](owner.control / "stop.json", {"stop": True})
            child = mock.Mock(pid=100)
            child.poll.return_value = None
            stopping, releasing = mock.Mock(), mock.Mock()
            stopping.is_set.return_value = releasing.is_set.return_value = False

            def release(_):
                child.wait.assert_called_once_with(timeout=15)
                self.assertEqual(self.journey["private_json"](owner.control / "runtime-stopped.json"), {"reaped": True})
                self.journey["private_json"](owner.control / "release.json", {"release": True})

            releasing.wait.side_effect = release
            with mock.patch.dict(self.globals, {"linux_process_identity": lambda pid: identity,
                                               "pidfd_exited": lambda _: False}), \
                    mock.patch.object(os, "pidfd_open", return_value=77, create=True), \
                    mock.patch.object(os, "close") as closed, mock.patch.object(signal, "signal"), \
                    mock.patch.object(self.globals["threading"], "Event", side_effect=[stopping, releasing]), \
                    mock.patch.object(subprocess, "Popen", return_value=child):
                self.journey["runtime_unit_helper"](owner.control)
            releasing.wait.assert_called_once_with(.1)
            closed.assert_called_once_with(77)

    def test_finite_helper_bounds_release_wait_and_rejects_invalid_release(self):
        for fault in ("timeout", "invalid", "owner_exit"):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as temp:
                owner, _ = self.finite_owner(Path(temp))
                runtime = owner.data / "bin/elastos"
                identity = {"pid": 900, "uid": 1001, "start_ticks": 71, "executable": "/fixture/python",
                            "device": 1, "inode": 2}
                self.journey["private_json"](owner.control / "launch.json", {
                    "owner": identity, "runtime": str(runtime), "runtime_sha256": self.journey["digest"](runtime),
                    "environment": {}, "address": "127.0.0.1:1", "log": str(owner.home / "private.log")})
                self.journey["private_json"](owner.control / "stop.json", {"stop": True})
                if fault == "invalid":
                    self.journey["private_json"](owner.control / "release.json", {"release": False})
                child = mock.Mock(pid=100)
                child.poll.return_value = None
                clock = mock.Mock()
                clock.monotonic.side_effect = [0, 1, 2, 63 if fault == "timeout" else 3]
                with mock.patch.dict(self.globals, {"linux_process_identity": lambda pid: identity, "time": clock,
                                                    "pidfd_exited": mock.Mock(side_effect=[False, fault == "owner_exit"])}), \
                        mock.patch.object(os, "pidfd_open", return_value=77, create=True), \
                        mock.patch.object(os, "close") as closed, mock.patch.object(signal, "signal"), \
                        mock.patch.object(subprocess, "Popen", return_value=child):
                    if fault == "invalid":
                        with self.assertRaisesRegex(RuntimeError, "helper release differs"):
                            self.journey["runtime_unit_helper"](owner.control)
                    else:
                        self.journey["runtime_unit_helper"](owner.control)
                child.wait.assert_called_once_with(timeout=15)
                closed.assert_called_once_with(77)
                self.assertEqual(self.journey["private_json"](owner.control / "runtime-stopped.json"), {"reaped": True})

    def finite_receipt(self, gib):
        flags = ("memory_refusal", "finite_home_recovery") if gib == 1 else (
            "model_busy", "model_stop", "model_stop_recovery", "model_idle_release", "model_idle_recovery",
            "model_engine_crash", "model_crash_recovery", "finite_home_recovery")
        actions = [] if gib == 1 else ["engine_running", "engine_stopped", "engine_running", "idle_stopped",
                                      "engine_running", "engine_running", "crash_engine", "engine_stopped", "engine_running"]
        processes = [{"pid": pid, "uid": os.getuid(), "start_ticks": pid + 700, "exe_sha256": "e" * 64, "role": role}
                     for pid, role in ((99, "helper"), (100, "runtime"), (101, "model_provider"))]
        if gib == 4:
            processes += [{"pid": pid, "uid": os.getuid(), "start_ticks": pid + 700,
                           "exe_sha256": ("e" if role == "guard" else "f") * 64, "role": role}
                          for pid, role in ((102, "guard"), (103, "llama_server"))]
        return {"installed_runtime_sha256": "e" * 64, "installed_model_provider_sha256": "e" * 64,
                "installed_engine": {"executable_sha256": "f" * 64}, "results": {name: "passed" for name in flags},
                "finite_memory": {"status": "passed", "limit_bytes": gib * 1024 ** 3, "swap_limit_bytes": 0,
                                  "tasks_limit": 512, "memory_peak_bytes": 512 * 1024 ** 2,
                                  "memory_events_before": {"oom": 0, "oom_kill": 0},
                                  "memory_events_after": {"oom": 0, "oom_kill": 0},
                                  "cgroup_empty": True, "whole_process_exit": True, "runtime_reaped": True,
                                  "observer_complete": True, "outside_runtime_unit": True, "browser_closed": True,
                                  "control_group": "/system.slice/elastos-ci-model-fixture.service",
                                  "native_membership_verified": True,
                                  "actual_runtime_pid": 100, "helper_main_pid": 99, "processes": processes,
                                  "actions": [{"action": action, "elapsed_seconds": 60 if action == "idle_stopped" else 1,
                                               "engine_pid": 103, "engine_start_ticks": 803} for action in actions]}}

    def test_finite_summary_refuses_missing_false_malformed_or_wrong_process_evidence(self):
        for gib in (1, 4):
            original = self.finite_receipt(gib)
            self.assertTrue(self.journey["finite_receipt_valid"](original, gib))
            for field, value in (("status", "failed"), ("limit_bytes", 0), ("swap_limit_bytes", "max"),
                                 ("tasks_limit", 0), ("memory_peak_bytes", 0), ("memory_peak_bytes", True),
                                 ("memory_peak_bytes", "100"), ("actual_runtime_pid", None),
                                 ("helper_main_pid", 100), ("processes", []),
                                 ("control_group", None), ("control_group", True),
                                 ("control_group", "/foreign/elastos-ci-model-fixture.service"),
                                 ("control_group", "/system.slice/elastos-ci-model-fixture .service"),
                                 ("control_group", "/system.slice/elastos-ci-model-../fixture.service"),
                                 ("native_membership_verified", 1), ("native_membership_verified", "true"),
                                 ("browser_closed", 1), ("browser_closed", "true"),
                                 *((key, False) for key in ("cgroup_empty", "whole_process_exit", "runtime_reaped",
                                                           "observer_complete", "outside_runtime_unit", "browser_closed",
                                                           "native_membership_verified"))):
                with self.subTest(gib=gib, field=field, value=value):
                    row = json.loads(json.dumps(original))
                    row["finite_memory"][field] = value
                    self.assertFalse(self.journey["finite_receipt_valid"](row, gib))
            for field in ("control_group", "native_membership_verified", "browser_closed"):
                with self.subTest(gib=gib, missing=field):
                    row = json.loads(json.dumps(original))
                    row["finite_memory"].pop(field)
                    self.assertFalse(self.journey["finite_receipt_valid"](row, gib))
            for fault in ("oom", "missing_flag", "wrong_role", "missing_birth", "wrong_runtime_hash", "engine_or_action"):
                with self.subTest(gib=gib, fault=fault):
                    row = json.loads(json.dumps(original))
                    finite = row["finite_memory"]
                    if fault == "oom":
                        finite["memory_events_after"]["oom_kill"] = 1
                    elif fault == "missing_flag":
                        row["results"].pop("finite_home_recovery")
                    elif fault == "wrong_role":
                        finite["processes"][1]["role"] = "helper"
                    elif fault == "missing_birth":
                        finite["processes"][1].pop("start_ticks")
                    elif fault == "wrong_runtime_hash":
                        finite["processes"][1]["exe_sha256"] = "a" * 64
                    elif gib == 1:
                        finite["processes"].append({"pid": 103, "role": "llama_server"})
                    else:
                        finite["actions"].pop()
                    self.assertFalse(self.journey["finite_receipt_valid"](row, gib))


class InstalledModelTimingTests(unittest.TestCase):
    def setUp(self):
        self.timing = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-model-timing.py"))

    def test_public_receipt_refuses_private_text_unknown_stages_and_invalid_times(self):
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "private.log"
            log.write_text("\n".join([
                "[model-provider] local timing stage=engine_ready elapsed_ms=123",
                "[model-provider] local timing stage=run_timeout elapsed_ms=120000",
                "[model-provider] local timing stage=operator_secret elapsed_ms=1",
                "[model-provider] local timing stage=engine_ready elapsed_ms=1 private=/operator/key",
                "[model-provider] local timing stage=engine_ready elapsed_ms=3600001",
                "[model-provider] local timing stage=engine_ready elapsed_ms=-1",
                "private model text and credentials",
            ]))
            self.assertEqual(self.timing["stage_timings"](log), [
                {"stage": "engine_ready", "elapsed_ms": 123},
                {"stage": "run_timeout", "elapsed_ms": 120000},
            ])

    def timing_fixture(self):
        stages = [{"stage": name, "elapsed_ms": value} for name, value in [
            ("run_started", 2), ("engine_ready", 400), ("generation_started", 500),
            ("first_delta", 600), ("stream_completed", 1000),
            ("generation_completed", 1020), ("terminal_applied", 1030)]]
        acknowledgements = [
            {"kind": "delta", "outcome": "applied", "elapsed_ms": 800, "duration_ms": 50},
            {"kind": "delta", "outcome": "applied", "elapsed_ms": 1020, "duration_ms": 20},
            {"kind": "terminal", "outcome": "applied", "elapsed_ms": 1030, "duration_ms": 10}]
        return stages, acknowledgements

    def test_generation_excludes_only_delta_waits_before_stream_terminal(self):
        result = self.timing["run_metrics"](*self.timing_fixture())
        self.assertEqual(result["status"], "complete")
        self.assertEqual(result["generation_wall_ms"], 500)
        self.assertEqual(result["generation_excluding_acknowledgement_ms"], 450)
        self.assertEqual(result["delta_acknowledgement_count"], 2)
        self.assertEqual(result["delta_acknowledgement_ms"], 70)
        self.assertEqual(result["delta_acknowledgement_max_ms"], 50)
        self.assertEqual(result["terminal_acknowledgement_ms"], 10)
        self.assertEqual(result["acknowledgement_total_ms"], 80)

    def test_incomplete_rejected_duplicate_reversed_and_timeout_timings_are_refused(self):
        for failure in ("missing", "rejected", "duplicate", "reversed", "timeout", "terminal", "overlap"):
            stages, ack = self.timing_fixture()
            if failure == "missing":
                stages.pop()
            elif failure == "rejected":
                ack[0]["outcome"] = "rejected"
            elif failure == "duplicate":
                stages.append(stages[0])
            elif failure == "reversed":
                stages[3]["elapsed_ms"] = 1100
            elif failure == "timeout":
                stages.append({"stage": "run_timeout", "elapsed_ms": 120000})
            elif failure == "terminal":
                ack.pop()
            else:
                ack[-1]["duration_ms"] = 100
            with self.subTest(failure=failure):
                self.assertEqual(self.timing["run_metrics"](stages, ack)["status"], "incomplete")

    def test_ack_receipt_refuses_private_fields_unknown_outcomes_and_invalid_durations(self):
        with tempfile.TemporaryDirectory() as temp:
            log = Path(temp) / "private.log"
            valid = "[model-provider] local acknowledgement kind=delta outcome=applied elapsed_ms=800 duration_ms=50"
            log.write_text("\n".join([valid, valid + " private=/operator/key",
                valid.replace("applied", "private_outcome"), valid.replace("duration_ms=50", "duration_ms=801"),
                valid.replace("elapsed_ms=800", "elapsed_ms=3600001"),
                valid.replace("duration_ms=50", "duration_ms=-1")]))
            self.assertEqual(self.timing["acknowledgement_timings"](log), [
                {"kind": "delta", "outcome": "applied", "elapsed_ms": 800, "duration_ms": 50}])

    def test_acknowledgements_outside_generation_or_in_wrong_order_are_refused(self):
        for failure in ("after_terminal", "before_generation", "overlap", "reversed", "across_stream"):
            stages, ack = self.timing_fixture()
            if failure == "after_terminal":
                ack.insert(2, {"kind": "delta", "outcome": "applied",
                               "elapsed_ms": 300000, "duration_ms": 200000})
            elif failure == "before_generation":
                ack[0].update(elapsed_ms=100, duration_ms=50)
            elif failure == "overlap":
                ack[1].update(elapsed_ms=1010, duration_ms=250)
            elif failure == "reversed":
                ack[0], ack[1] = ack[1], ack[0]
            else:
                ack[1].update(elapsed_ms=1020, duration_ms=30)
            with self.subTest(failure=failure):
                self.assertEqual(self.timing["run_metrics"](stages, ack)["status"], "incomplete")

    def test_spread_requires_three_same_candidate_installed_passes(self):
        row = {"candidate": "c" * 40, "source_tree": "d" * 40,
               "installed_runtime_sha256": "a" * 64, "installed_model_provider_sha256": "b" * 64,
               "results": {"installed_runtime_reply": "passed"},
               "model_timing": {"durations": self.timing["run_metrics"](*self.timing_fixture())}}
        rows = [json.loads(json.dumps(row)) for _ in range(3)]
        rows[1]["model_timing"]["durations"]["engine_ready_ms"] = 300
        rows[2]["model_timing"]["durations"]["engine_ready_ms"] = 450
        spread = self.timing["timing_spread"](rows, 3)
        self.assertEqual(spread["durations_ms"]["engine_ready_ms"],
                         {"values": [400, 300, 450], "min": 300, "max": 450, "spread": 150})
        self.assertEqual(self.timing["timing_spread"](rows[:2], 3)["status"], "incomplete")
        for failure in ("candidate", "reply", "timing"):
            altered = json.loads(json.dumps(rows))
            if failure == "candidate":
                altered[1]["installed_model_provider_sha256"] = "different"
            elif failure == "reply":
                altered[1]["results"]["installed_runtime_reply"] = "failed"
            else:
                altered[1]["model_timing"]["durations"]["status"] = "incomplete"
            with self.subTest(failure=failure):
                self.assertEqual(self.timing["timing_spread"](altered, 3)["status"], "incomplete")

        for field in ("candidate", "source_tree", "installed_runtime_sha256", "installed_model_provider_sha256"):
            for value in (None, "", "not-a-digest"):
                altered = json.loads(json.dumps(rows))
                for record in altered:
                    record[field] = value
                with self.subTest(field=field, value=value):
                    self.assertEqual(self.timing["timing_spread"](altered, 3)["status"], "incomplete")
        for value in (None, -1, "500", True, 3600001):
            altered = json.loads(json.dumps(rows))
            altered[1]["model_timing"]["durations"]["generation_wall_ms"] = value
            with self.subTest(value=value):
                self.assertEqual(self.timing["timing_spread"](altered, 3)["status"], "incomplete")

    def test_summary_requires_three_current_candidate_passes_and_preserves_failure(self):
        shell = (WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh").read_text()
        source = shell.split('python3 - "$EVIDENCE" "$DATA" <<\'PY\'\n', 1)[1].split('\nPY\n', 1)[0]
        for failure in (None, "missing", "candidate", "reply", "absence", "refusal", "refusal_reason", "engine_process"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                _, data, _, _ = InstalledJourneyTests().fixture(root, "macos")
                runtime = data / "bin/elastos"
                journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
                identities = {"installed_model_provider_sha256": journey["digest"](data / "bin/model-provider"),
                              "source_components_sha256": journey["digest"](data / "components.json")}
                row = {**identities, "candidate": "c" * 40, "source_tree": "d" * 40, "installed_engine": journey["engine_receipt"](data),
                       "installed_runtime_sha256": hashlib.sha256(runtime.read_bytes()).hexdigest(),
                       "results": {name: "passed" for name in ["home_screenshots", "model_package_admission", "installed_runtime_reply", "model_timing_observer", "process_cleanup", "disk_reserve"]},
                       "model_timing": {"durations": self.timing["run_metrics"](*self.timing_fixture())}}
                for index in range(1, 4):
                    if failure == "missing" and index == 3:
                        continue
                    value = json.loads(json.dumps(row))
                    if failure == "candidate":
                        value["candidate"] = "a" * 40
                    if failure == "reply" and index == 1:
                        value["results"]["installed_runtime_reply"] = "failed"
                    evidence = root / f"run-{index}"
                    evidence.mkdir()
                    (evidence / "installed-journeys.json").write_text(json.dumps(value))
                absent = root / "engine-absent-home"
                absent.mkdir()
                (absent / "installed-journeys.json").write_text(json.dumps({
                    **identities, "candidate": "c" * 40, "source_tree": "d" * 40, "engine_absent": True,
                    "installed_runtime_sha256": hashlib.sha256(runtime.read_bytes()).hexdigest(),
                    "dispatch_unavailable_reason": None if failure == "refusal_reason" else "source_engine_required",
                    "process_cleanup": {"before": {"llama_server": 1 if failure == "engine_process" else 0}},
                    "results": {name: "failed" if (failure == "absence" and name == "engine_absent_home") or (failure == "refusal" and name == "engine_absent_refusal") else "passed"
                                for name in ("engine_absent_home", "engine_absent_refusal", "home_screenshots", "process_cleanup", "disk_reserve")}}))
                with mock.patch.object(sys, "argv", ["summary", str(root), str(data)]), \
                        mock.patch.object(sys, "platform", "darwin"), \
                        mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(root / "summary.md")}), \
                        mock.patch.object(subprocess, "check_output", side_effect=["c" * 40 + "\n", "d" * 40 + "\n"]):
                    if failure:
                        with self.assertRaises(SystemExit):
                            exec(compile(source, "installed-summary", "exec"), {})
                    else:
                        exec(compile(source, "installed-summary", "exec"), {})
                result = json.loads((root / "core-summary.json").read_text())
                self.assertEqual(result["model_timing_spread"]["status"],
                                 "incomplete" if failure in ("missing", "candidate", "reply") else "complete")
                if failure == "reply":
                    self.assertEqual(result["results"]["installed_runtime_reply"], "failed or not run")
                if failure in ("absence", "refusal", "refusal_reason", "engine_process"):
                    self.assertEqual(result["results"]["engine_absent_home"], "failed or not run")
                summary = (root / "summary.md").read_text()
                self.assertIn(f"Source tree: `{'d' * 40}`", summary)
                self.assertIn("OS file cache can warm", summary)
                if not failure:
                    self.assertIn("acknowledgement_total_ms", summary)
                    self.assertIn("Delta Applied count", summary)

    def test_linux_summary_requires_carrier_get_only_where_the_workflow_names_it(self):
        shell = (WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh").read_text()
        source = shell.split('python3 - "$EVIDENCE" "$DATA" <<\'PY\'\n', 1)[1].split('\nPY\n', 1)[0]
        for carrier, carrier_get in (("true", "passed"), ("true", None), ("false", None)):
            with self.subTest(carrier=carrier, carrier_get=carrier_get), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                _, data, _, _ = InstalledJourneyTests().fixture(root, "linux")
                journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
                identities = {"candidate": "c" * 40, "source_tree": "d" * 40,
                              "installed_runtime_sha256": journey["digest"](data / "bin/elastos"),
                              "installed_model_provider_sha256": journey["digest"](data / "bin/model-provider"),
                              "source_components_sha256": journey["digest"](data / "components.json")}
                results = {name: "passed" for name in ("home_screenshots", "model_package_admission", "installed_runtime_reply",
                                                       "model_timing_observer", "process_cleanup", "disk_reserve")}
                if carrier_get:
                    results["carrier_get"] = carrier_get
                (root / "installed-journeys.json").write_text(json.dumps({
                    **identities, "installed_engine": journey["engine_receipt"](data), "results": results}))
                (root / "engine-absent-home").mkdir()
                (root / "engine-absent-home/installed-journeys.json").write_text(json.dumps({
                    **identities, "engine_absent": True, "dispatch_unavailable_reason": "source_engine_required",
                    "process_cleanup": {"before": {"llama_server": 0}},
                    "results": {name: "passed" for name in ("engine_absent_home", "engine_absent_refusal", "home_screenshots",
                                                            "process_cleanup", "disk_reserve")}}))
                with mock.patch.object(sys, "argv", ["summary", str(root), str(data)]), \
                        mock.patch.object(sys, "platform", "linux"), \
                        mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(root / "summary.md"), "CI_CARRIER_GET": carrier}), \
                        mock.patch.object(subprocess, "check_output", side_effect=["c" * 40 + "\n", "d" * 40 + "\n"]):
                    if carrier == "true" and not carrier_get:
                        with self.assertRaises(SystemExit):
                            exec(compile(source, "installed-summary", "exec"), {})
                    else:
                        exec(compile(source, "installed-summary", "exec"), {})
                summary = json.loads((root / "core-summary.json").read_text())["results"]
                if carrier == "true":
                    self.assertEqual(summary["carrier_get"], "passed" if carrier_get else "failed or not run")
                else:
                    self.assertNotIn("carrier_get", summary)

    def test_only_the_linux_x86_job_gets_the_reply_package_from_a_holder(self):
        for job in ("source-home-linux", "source-home-macos"):
            for name in ("installed Marketplace Get and Assistant reply", "summarize installed model journey"):
                step, = [step for step in steps(job) if step.startswith(f"name: {name}\n")]
                if job == "source-home-linux":
                    self.assertEqual(field(step, "CI_CARRIER_GET"), "${{ matrix.os == 'ubuntu-24.04' }}")
                    self.assertEqual(field(step, "CI_FINITE_MEMORY"), "${{ matrix.os == 'ubuntu-24.04' }}")
                else:
                    self.assertNotIn("CI_CARRIER_GET", step)
                    self.assertNotIn("CI_FINITE_MEMORY", step)
        self.assertEqual(re.search(r"(?m)^        os: \[([^\]]+)\]$", JOBS["source-home-linux"])[1], "ubuntu-24.04")

    def test_probe_refuses_wrong_alias_malformed_and_oversize_responses(self):
        alias = "a" * 32
        cases = [(json.dumps({"data": [{"id": alias}]}).encode(), "matching_alias"),
                 (json.dumps({"data": [{"id": "b" * 32}]}).encode(), "wrong_alias"),
                 (b"private malformed response", "unavailable"),
                 (b"x" * 16385, "oversize")]
        for body, expected in cases:
            with self.subTest(expected=expected), mock.patch("socket.socket") as connect:
                wire = b"HTTP/1.1 200 OK\r\nContent-Length: " + str(len(body)).encode() + b"\r\n\r\n" + body
                connect.return_value.__enter__.return_value.recv.side_effect = [wire, b""]
                self.assertEqual(self.timing["matching_alias"]("/owned.engine.sock", alias), expected)

    def test_observer_refuses_unrelated_processes_and_foreign_engine_paths(self):
        data = Path("/isolated/data/elastos")
        engine = (f"{data}/libexec/llama-server -m /model --host /owned.engine.sock "
                  f"--ctx-size 4096 --alias {'a' * 32}")
        rows = {2: (1, str(data / "bin/model-provider")),
                3: (2, str(data / "bin/model-provider") + " --internal-local-llama-guard"),
                4: (3, engine)}
        observe = self.timing["owned_engine"]
        self.assertEqual(list(observe(rows, 1, data)), [(4, "/owned.engine.sock", "a" * 32)])
        self.assertEqual(list(observe(rows, 9, data)), [])
        self.assertEqual(list(observe({**rows, 4: (3, engine.replace(str(data), "/foreign"))}, 1, data)), [])
        self.assertEqual(list(observe({**rows, 4: (2, engine)}, 1, data)), [])
        for guard in ("/foreign/guard --internal-local-llama-guard",
                      str(data / "bin/model-provider") + " --internal-local-llama-guard extra"):
            with self.subTest(guard=guard):
                self.assertEqual(list(observe({**rows, 3: (2, guard)}, 1, data)), [])


if __name__ == "__main__":
    unittest.main()
