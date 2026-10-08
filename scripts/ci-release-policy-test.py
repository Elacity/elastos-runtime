#!/usr/bin/env python3
"""Check CI release/cache decisions without builds, Docker, or publication."""
import os
import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import tarfile
import tempfile
import textwrap
import unittest
from unittest import mock


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/ci.yml"
SOURCE = WORKFLOW.read_text()
CACHE_ACTION = WORKFLOW.parents[1] / "actions/rust-compile-cache/action.yml"
LOCAL_CACHE_ACTION = "./.github/actions/rust-compile-cache"
UNIT_CACHE_JOBS = {"lint", "test-elastos", "test-capsules"}
SOURCE_HOME_CACHE_JOBS = {"source-home-linux", "source-home-linux-arm64", "source-home-macos"}
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
    expression = re.sub(r"(?:github|inputs|env|steps|matrix)\.[\w.-]+",
                        lambda match: repr(context[match[0]]), expression)
    return eval(expression, {"__builtins__": {}},
                {"startsWith": str.startswith, "true": True, "false": False})


def validate_action_pins(source, cache_action):
    commits = {}
    for index, block in enumerate((source, cache_action)):
        uses = re.findall(r"(?m)^\s+(?:- )?uses: (\S+)(.*)$", block)
        if not uses:
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


def run_compile_cache_fixture(source, mode, download_status=0, server_status=0,
                              platform=("Linux", "X64"), compiler="fixture rustc",
                              lockfile="fixture lock", archive=None, checksum=None):
    """Execute action shell control flow with local transport/server fixtures."""
    action_steps = re.split(r"(?m)^    - ", source.split("  steps:\n", 1)[1])[1:]
    install = textwrap.dedent(action_steps[0].split("      run: |\n", 1)[1])
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        shims = root / "shims"
        shims.mkdir()
        (root / "Cargo.lock").write_text(lockfile)
        for name, body in {
            "curl": 'printf "%s\\n" "$@" > "$DOWNLOAD_LOG"\nwhile [ "$1" != -o ]; do shift; done\nprintf fixture > "$2"\nexit "$DOWNLOAD_STATUS"\n',
            "rustc": '[ "$*" = -vV ] || exit 99\nprintf "%s\\n" "$COMPILER_ID"\n',
            "tar": 'exit 0\n',
            "sccache": '[ "$*" = --start-server ] || exit 99\necho started >> "$SERVER_LOG"\nexit "$SERVER_STATUS"\n',
        }.items():
            path = shims / name
            path.write_text("#!/bin/bash\n" + body)
            path.chmod(0o700)
        # Hash compiler inputs for real; the corrupt-archive test checks real verification.
        for name in ("sha256sum", "shasum"):
            path = shims / name
            path.write_text(f"#!{sys.executable}\n" + textwrap.dedent("""\
                import hashlib, os, pathlib, sys
                if '-c' in sys.argv:
                    pathlib.Path(os.environ['CHECKSUM_LOG']).write_bytes(sys.stdin.buffer.read())
                else:
                    print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest() + '  -')
                """))
            path.chmod(0o700)
        env = {**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
               "RUNNER_OS": platform[0], "RUNNER_ARCH": platform[1], "RUNNER_TEMP": str(root),
               "GITHUB_PATH": str(root / "path"), "GITHUB_ENV": str(root / "env"),
               "CACHE_RW_MODE": mode,
               "DOWNLOAD_STATUS": str(download_status), "SERVER_STATUS": str(server_status),
               "SERVER_LOG": str(root / "server"), "COMPILER_ID": compiler,
               "DOWNLOAD_LOG": str(root / "download"),
               "CHECKSUM_LOG": str(root / "checksum")}
        env.pop("RUSTC_WRAPPER", None)
        result = subprocess.run(["bash", "-e", "-c", install], env=env,
                                capture_output=True, text=True, cwd=root, timeout=10)
        if archive is not None:
            assert result.returncode == 0, result.stderr
            assert f"/v0.18.0/{archive}.tar.gz" in (root / "download").read_text()
            assert (root / "checksum").read_text() == f"{checksum}  {root}/rust-compile-cache/{archive}.tar.gz\n"
        def exports():
            return dict(line.split("=", 1) for line in (root / "env").read_text().splitlines()) \
                if (root / "env").exists() else {}
        installed_exports = exports()
        env.update(installed_exports)
        # Execute the action's activation guard and shell, including skipped installs.
        for step in action_steps[1:]:
            if "      run: |\n" not in step:
                continue
            if evaluate(field(step, "if"), {"steps.install.outcome":
                                            "success" if result.returncode == 0 else "failure"}):
                script = textwrap.dedent(step.split("      run: |\n", 1)[1])
                subprocess.run(["bash", "-e", "-c", script], env=env, check=True,
                               capture_output=True, text=True, timeout=10)
        return result.returncode, installed_exports, exports(), (root / "server").exists()


# event, workflow ref, ref type, checkout override, caches, publication
CASES = [
    ("push", "refs/heads/main", "branch", "", True, False),
    ("push", "refs/heads/develop", "branch", "", True, False),
    ("push", "refs/heads/v-work", "branch", "", True, False),
    ("push", "refs/tags/v0.7.1", "tag", "", False, True),
    ("push", "refs/tags/candidate", "tag", "", False, False),
    ("pull_request", "refs/pull/1/merge", "branch", "", True, False),
    ("merge_group", "refs/heads/gh-readonly-queue/develop/pr-1", "branch", "", True, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "", True, False),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "v0.7.1", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/tags/v0.7.1", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/heads/work", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "a" * 40, False, False),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "main", False, False),
]
# Only these branch pushes may write shared caches.
SAVING_REFS = {"refs/heads/develop", "refs/heads/main"}
NO_CACHE_HIT = {"steps.engine-cache.outputs.cache-hit": "false",
                "steps.previous-release-cache.outputs.cache-hit": "false",
                "steps.kubo-cache.outputs.cache-hit": "false",
                "steps.apt-prerequisites.outputs.changed": "true"}


def job_runs(job, context, workflow_jobs=JOBS):
    guard = re.search(r"(?m)^    if: (.*)$", workflow_jobs[job])
    return evaluate(guard[1], context) if guard else True


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


def validate_cache_guards(source):
    workflow_jobs = jobs(source)
    for job in workflow_jobs:
        for step in steps(job, workflow_jobs):
            matches = list(CACHE_RE.finditer(step))
            if not matches:
                continue
            # Buildx shell cache arguments live inside this explicit cache-only branch.
            shell_guards = list(re.finditer(
                r'(?ms)^\s*if \[\[ "\$\{CI_(?:USE|SAVE)_CACHE\}" == "true" \]\]; then\n'
                r'(.*?)^\s*fi\s*$', step))
            shell_guards = [guard for guard in shell_guards
                            if not re.search(r"(?m)^\s*(?:else|elif)\b", guard[1])]
            remaining = [match for match in matches
                         if not (match[0] == "type=gha" and any(
                             guard.start(1) <= match.start() < guard.end(1)
                             for guard in shell_guards))]
            if not remaining:
                continue
            guard = field(step, "if")
            if "env.CI_USE_CACHE == 'true'" not in guard:
                raise AssertionError(f"unguarded cache in {job}")
            for event, ref, ref_type, override, cached, _ in CASES:
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


def validate_jetson_package_lifecycle(source):
    job_steps = steps("source-home-linux-arm64", jobs(source))
    package, = [step for step in job_steps if step.startswith("name: build and package release binaries\n")]
    verify, = [step for step in job_steps if step.startswith("name: verify Jetson release compatibility\n")]
    if job_steps.index(package) >= job_steps.index(verify):
        raise AssertionError("Jetson verification requires an earlier package step")
    if "scripts/package-release-binaries.sh" not in package:
        raise AssertionError("Jetson verification requires the package producer")
    for event, ref, ref_type, override, _, _ in CASES:
        for runner in ("ubuntu-24.04", "ubuntu-22.04-arm"):
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override, "matrix.os": runner}
            if evaluate(field(verify, "if"), context) and not evaluate(field(package, "if"), context):
                raise AssertionError(f"Jetson verification lacks its package on {event} {runner}")


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
        for event, ref, ref_type, override, _, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override}
            expected = event in ("pull_request", "merge_group") or (event == "push" and ref == "refs/heads/develop")
            if evaluate(field(profile, "if"), context) != expected:
                raise AssertionError(f"journey profile changes shipped build: {job} {event} {ref}")
        cache, = [step for step in job_steps if "Swatinem/rust-cache@" in step]
        if field(cache, "cache-targets") != "false" or "cache-directories:" in cache or "source-home-registry" not in cache:
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
            if job in ("release", "cancel-on-failure"):
                continue
            guard = re.search(r"(?m)^    if: (.*)$", JOBS[job])
            if guard:
                self.assertTrue(evaluate(guard[1], context), job)
            checkout, = [step for step in steps(job) if "uses: actions/checkout@" in step]
            self.assertEqual(field(checkout, "ref"),
                             "${{ github.event_name == 'workflow_dispatch' && inputs.ref || github.sha }}")
        for event, ref, ref_type, override, _, _ in CASES:
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
        setup_source = (WORKFLOW.parents[1] / "actions/rust-compile-cache/action.yml").read_text()
        self.assertIn("SCCACHE_GHA_ENABLED=on", setup_source)
        self.assertIn("RUSTC_WRAPPER=sccache", setup_source)
        self.assertIn("CARGO_INCREMENTAL=0", setup_source)
        self.assertNotIn("hashFiles", setup_source)
        self.assertIn("SCCACHE_GHA_RW_MODE=", setup_source)
        for name in ("ACTIONS_RESULTS_URL", "ACTIONS_RUNTIME_TOKEN"):
            self.assertIn(f"core.exportVariable('{name}', process.env.{name}", setup_source)
        uncached = SOURCE.split("\njobs:\n", 1)[0] + "\n".join(
            JOBS[job] for job in JOBS if job not in expected)
        self.assertNotRegex(uncached, r"(?i)sccache|RUSTC_WRAPPER|rust-compile-cache")

    def test_compile_free_source_gate_keeps_one_nested_installer_suite(self):
        source_gate = JOBS["source-gate"]
        self.assertNotRegex(source_gate, r"(?i)sccache|RUSTC_WRAPPER|rust-compile-cache")
        self.assertNotIn("python3 scripts/install-bootstrap-test.py", source_gate)
        self.assertEqual(source_gate.count("python3 scripts/release-platform-input-test.py"), 1)
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
        self.assertLess(setup_source.index("tar -xzf"), setup_source.index("RUSTC_WRAPPER=sccache"))

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
                           "INSTALL_VERSION": "1.2.3", "UPDATE_VERSION": "1.2.4", "BUILD_CALLS": str(calls)}
                    result = subprocess.run(["bash", "-c", script], cwd=root, env=env,
                                            capture_output=True, text=True, timeout=10)
                    self.assertEqual(result.returncode == 0, existing is None, result.stderr)
                    if existing:
                        self.assertFalse(calls.exists())
                        self.assertTrue(output.exists() or output.is_symlink())
                    else:
                        built = [json.loads(line) for line in calls.read_text().splitlines()]
                        self.assertEqual(len(built), 2)
                        self.assertEqual({item['target'] for item in built}, {str(target)})
                        self.assertEqual({item['build'] for item in built}, {str(build_dir)})
                        self.assertEqual(built[0]['args'], ['--version', '1.2.3', '--output', str(release / 'inputs/N')])
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

    def test_sccache_namespace_survives_lock_edits_and_changes_with_compiler(self):
        source = CACHE_ACTION.read_text()
        def namespace(**kwargs):
            status, _, exported, _ = run_compile_cache_fixture(source, "READ_ONLY", **kwargs)
            self.assertEqual(status, 0)
            return exported["SCCACHE_GHA_VERSION"]
        original = namespace()
        self.assertIn(hashlib.sha256(b"fixture rustc\n").hexdigest(), original)
        self.assertEqual(original, namespace(lockfile="changed dependency resolution"))
        self.assertNotEqual(original, namespace(compiler="changed rustc -vV"))

    def test_sccache_platform_archives_are_checked_before_activation(self):
        source = CACHE_ACTION.read_text()
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
                _, installed, final, started = run_compile_cache_fixture(
                    source, "READ_ONLY", platform=platform,
                    archive=f"sccache-v0.18.0-{target}", checksum=checksum)
                self.assertNotIn("RUSTC_WRAPPER", installed)
                self.assertTrue(started)
                self.assertEqual(final["RUSTC_WRAPPER"], "sccache")
                for download, server in ((22, 0), (0, 1)):
                    _, _, failed, _ = run_compile_cache_fixture(
                        source, "READ_ONLY", download, server, platform=platform)
                    self.assertNotIn("RUSTC_WRAPPER", failed)
        status, _, exported, started = run_compile_cache_fixture(
            source, "READ_ONLY", platform=("Windows", "X64"))
        self.assertNotEqual(status, 0)
        self.assertNotIn("RUSTC_WRAPPER", exported)
        self.assertFalse(started)

    def test_sccache_jobs_keep_registry_caches_without_target_artifacts(self):
        for job in ("source-gate", "lint", "test-elastos", "test-capsules"):
            for step in steps(job):
                if "Swatinem/rust-cache@" in step:
                    self.assertEqual(field(step, "cache-targets"), "false", job)
                    self.assertEqual(field(step, "cache-bin"), "false", job)
                    self.assertNotIn("cache-directories:", step, job)
                self.assertNotRegex(step, r"uses: actions/cache(?:/restore|/save)?@", job)
        for job in ("lint", "test-elastos", "test-capsules"):
            self.assertIn("uses: Swatinem/rust-cache@", JOBS[job])

    def test_sccache_failures_leave_normal_builds_enabled(self):
        source = CACHE_ACTION.read_text()
        install = source.split("    - name:", 2)[1]
        for download, server, expected in ((22, 0, False), (0, 1, False), (0, 0, True)):
            with self.subTest(download=download, server=server):
                status, installed, final, started = run_compile_cache_fixture(
                    source, "READ_ONLY", download, server)
                self.assertEqual(status == 0, download == 0)
                self.assertNotIn("RUSTC_WRAPPER", installed)
                self.assertEqual(started, download == 0)
                self.assertEqual(final.get("RUSTC_WRAPPER"), "sccache" if expected else None)
        self.assertEqual(field(install, "continue-on-error"), "true")

    def test_sccache_read_only_mode_is_enforced_for_every_event(self):
        def verify(source):
            expression = field(source, "CACHE_RW_MODE")
            for event, ref, ref_type, override, _, _ in CASES:
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override}
                context["env.CI_SAVE_CACHE"] = str(evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)).lower()
                mode = evaluate(expression, context)
                expected = "READ_WRITE" if event == "push" and ref in SAVING_REFS else "READ_ONLY"
                self.assertEqual(mode, expected, f"cache mode on {event} {ref}")
                _, _, exported, _ = run_compile_cache_fixture(source, mode)
                self.assertEqual(exported.get("SCCACHE_GHA_RW_MODE"), expected)
        source = CACHE_ACTION.read_text()
        verify(source)
        forced_write = source.replace(field(source, "CACHE_RW_MODE"), "${{ 'READ_WRITE' }}", 1)
        # Keep the original expression in a comment to catch substring-only checks.
        forced_write += "\n# " + field(source, "CACHE_RW_MODE") + "\n"
        with self.assertRaises(AssertionError):
            verify(forced_write)

    def test_sccache_installer_refuses_corrupt_bytes_before_extraction(self):
        source = (WORKFLOW.parents[1] / "actions/rust-compile-cache/action.yml").read_text()
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
                                        env={**os.environ, "PATH": str(shims) + os.pathsep + os.environ["PATH"],
                                             "RUNNER_OS": platform[0], "RUNNER_ARCH": platform[1], "RUNNER_TEMP": str(root),
                                             "GITHUB_PATH": str(root / "path"), "GITHUB_ENV": str(root / "env"),
                                             "CACHE_RW_MODE": "READ_ONLY"})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("FAILED", result.stdout)
                self.assertFalse((root / "extracted").exists())
                self.assertFalse((root / "path").exists())
                self.assertFalse((root / "env").exists())

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
                    self.assertEqual(env["SCCACHE_MULTILEVEL_CHAIN"], "disk,gha")
                    self.assertEqual(Path(env["SCCACHE_DIR"]), root / "source-home-sccache")
                    self.assertEqual(env["SCCACHE_CACHE_SIZE"], "2G")
                    self.assertEqual(env["SCCACHE_LOCAL_RW_MODE"], "READ_WRITE")
                    self.assertEqual(env["SCCACHE_MULTILEVEL_WRITE_ERROR_POLICY"], "l0")
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

    def test_event_ref_matrix_controls_publication_and_every_cache_action(self):
        validate_cache_guards(SOURCE)
        caches = [(job, step) for job in JOBS for step in steps(job)
                  if CACHE_RE.search(step) and "type=gha" not in step]
        self.assertEqual(len(caches), 27)
        for event, ref, ref_type, override, cached, publish in CASES:
            with self.subTest(event=event, ref=ref, override=override):
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override}
                use_cache = evaluate(field(SOURCE, "CI_USE_CACHE"), context)
                self.assertEqual(use_cache, cached)
                self.assertEqual(evaluate(field(JOBS["release"], "if"), context), publish)
                context["env.CI_USE_CACHE"] = str(use_cache).lower()
                save = evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)
                self.assertEqual(save, event == "push" and ref in SAVING_REFS)
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
        for event, ref, _, _, _, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref}
            eligible = event in ("pull_request", "merge_group", "workflow_dispatch") or (event == "push" and ref in SAVING_REFS)
            self.assertEqual(evaluate(field(JOBS["custody-harness-smoke"], "if"), context), eligible)
            self.assertEqual(evaluate(field(filter_step, "if"), context), event == "pull_request")
            if not eligible:
                continue
            for changed in ("true", "false", ""):
                values = {**context, "steps.filter.outputs.custody": changed,
                          "env.CI_SAVE_CACHE": str(event == "push" and ref in SAVING_REFS).lower()}
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

    def test_only_pr_runs_share_a_cancellable_concurrency_group(self):
        concurrency = SOURCE.split("\nconcurrency:\n", 1)[1].split("\nenv:\n", 1)[0]
        group = field(concurrency, "group")
        for event, ref, _, _, _, _ in CASES:
            groups = []
            for run_id in ("101", "102", "103"):
                context = {"github.workflow": "CI", "github.event_name": event,
                           "github.ref": ref, "github.run_id": run_id}
                groups.append(re.sub(r"\$\{\{\s*(.*?)\s*\}\}", lambda match: str(evaluate(match[1], context)), group))
                self.assertEqual(evaluate(field(concurrency, "cancel-in-progress"), context), event == "pull_request")
            self.assertEqual(len(set(groups)), 1 if event == "pull_request" else 3)

    def test_jetson_package_exists_before_verification_on_every_event(self):
        validate_jetson_package_lifecycle(SOURCE)
        guard = "if: matrix.os == 'ubuntu-22.04-arm' || github.ref_type == 'tag' || github.ref == 'refs/heads/main' || github.event_name == 'workflow_dispatch'"
        self.assertIn(guard, JOBS["source-home-linux"])
        regressed = SOURCE.replace(guard, "if: github.event_name != 'pull_request'", 1)
        with self.assertRaisesRegex(AssertionError, "lacks its package on pull_request ubuntu-22.04-arm"):
            validate_jetson_package_lifecycle(regressed)
        omitted = SOURCE.replace("scripts/package-release-binaries.sh", "echo package omitted", 1)
        with self.assertRaisesRegex(AssertionError, "requires the package producer"):
            validate_jetson_package_lifecycle(omitted)

    def test_package_runs_follow_uploads_except_the_arm_compatibility_gate(self):
        for job, runner in (("source-home-linux", "ubuntu-24.04"),
                            ("source-home-linux-arm64", "ubuntu-22.04-arm"),
                            ("source-home-macos", "macos-14")):
            package, = [step for step in steps(job) if step.startswith("name: build and package release binaries\n")]
            upload, = [step for step in steps(job) if step.startswith("name: upload release binaries\n")]
            for event, ref, ref_type, override, _, _ in CASES:
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override, "matrix.os": runner}
                expected_upload = ref_type == "tag" or ref == "refs/heads/main" or event == "workflow_dispatch"
                with self.subTest(job=job, event=event, ref=ref, override=override):
                    self.assertEqual(evaluate(field(upload, "if"), context), expected_upload)
                    self.assertEqual(evaluate(field(package, "if"), context),
                                     expected_upload or job == "source-home-linux-arm64")

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
        nested = re.sub(r"(uses: actions/github-script)@[0-9a-f]{40}", r"\1@v7", action)
        with self.assertRaisesRegex(AssertionError, "mutable action"):
            validate_action_pins(SOURCE, nested)

    def test_source_home_cache_action_and_modes_follow_actual_event_policy(self):
        action = CACHE_ACTION.read_text()
        expected_jobs = SOURCE_HOME_CACHE_JOBS
        self.assertEqual({job for job in JOBS if any(f"uses: {LOCAL_CACHE_ACTION}" in step
                                                   for step in steps(job))},
                         UNIT_CACHE_JOBS | expected_jobs)
        for job in expected_jobs:
            environment = JOBS[job].split("    steps:", 1)[0]
            # The shared action owns remote permissions from the actual event policy.
            self.assertNotIn("SCCACHE_GHA_RW_MODE:", environment)
            setup, = [step for step in steps(job) if f"uses: {LOCAL_CACHE_ACTION}" in step]
            job_source = "\n".join(steps(job))
            self.assertLess(job_source.index("uses: dtolnay/rust-toolchain@"), job_source.index(setup))
            self.assertLess(job_source.index(setup), job_source.index("scripts/setup-source-home.sh"))
            statistics, = [step for step in steps(job) if step.startswith("name: Rust compile cache statistics\n")]
            self.assertEqual(field(statistics, "if"), "always() && env.RUSTC_WRAPPER == 'sccache'")
            self.assertIn("sccache --show-stats", statistics)
            self.assertIn("sccache --show-stats --stats-format=json", statistics)

        def verify(candidate):
            expression = field(candidate, "CACHE_RW_MODE")
            for event, ref, ref_type, override, cached, _ in CASES:
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override}
                context["env.CI_USE_CACHE"] = str(evaluate(field(SOURCE, "CI_USE_CACHE"), context)).lower()
                context["env.CI_SAVE_CACHE"] = str(evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)).lower()
                mode = evaluate(expression, context)
                expected = "READ_WRITE" if event == "push" and ref in SAVING_REFS else "READ_ONLY"
                self.assertEqual(mode, expected, f"cache mode on {event} {ref} {override}")
                for job in expected_jobs:
                    setup, = [step for step in steps(job) if f"uses: {LOCAL_CACHE_ACTION}" in step]
                    self.assertEqual(evaluate(field(setup, "if"), context), cached, job)
                if cached:
                    _, _, exported, _ = run_compile_cache_fixture(candidate, mode)
                    self.assertEqual(exported["SCCACHE_GHA_RW_MODE"], expected)
                    self.assertEqual(exported["RUSTC_WRAPPER"], "sccache")
                    self.assertEqual(exported["CARGO_INCREMENTAL"], "0")
        verify(action)
        forced_write = action.replace(field(action, "CACHE_RW_MODE"), "${{ 'READ_WRITE' }}", 1)
        forced_write += "\n# " + field(action, "CACHE_RW_MODE") + "\n"
        with self.assertRaises(AssertionError):
            verify(forced_write)

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
        self.assertEqual(field(action.split("    - name:", 2)[1], "continue-on-error"), "true")
        for platform, target, checksum in releases:
            with self.subTest(platform=platform):
                _, installed, final, started = run_compile_cache_fixture(
                    action, "READ_ONLY", platform=platform,
                    archive=f"sccache-v0.18.0-{target}", checksum=checksum)
                self.assertNotIn("RUSTC_WRAPPER", installed)
                self.assertTrue(started)
                self.assertEqual(final["RUSTC_WRAPPER"], "sccache")
                for download, server in ((22, 0), (0, 1)):
                    status, _, failed, attempted = run_compile_cache_fixture(
                        action, "READ_ONLY", download, server, platform=platform)
                    self.assertEqual(status == 0, download == 0)
                    self.assertEqual(attempted, download == 0)
                    self.assertNotIn("RUSTC_WRAPPER", failed)
        status, _, exported, started = run_compile_cache_fixture(
            action, "READ_ONLY", platform=("Windows", "X64"))
        self.assertNotEqual(status, 0)
        self.assertNotIn("RUSTC_WRAPPER", exported)
        self.assertFalse(started)

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
                                                        "GITHUB_PATH": str(root / "path"), "GITHUB_ENV": str(root / "env"),
                                                        "CACHE_RW_MODE": "READ_ONLY"})
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

    def test_buildx_imports_and_exports_layers_only_when_cache_is_enabled(self):
        step, = [step for step in steps("custody-harness-smoke")
                 if "docker buildx build" in step]
        script = textwrap.dedent(step.split("        run: |\n", 1)[1])
        # Execute the actual workflow shell with a recording function, not Docker.
        stub = 'docker() { printf "%s\\0" "$@"; }\n'
        base = ["buildx", "build", "-f", "deploy/custody-host/Dockerfile",
                "-t", "elastos-custody-host:latest"]
        # Derive cache settings from actual events, then apply the job and step guards.
        for event, ref, ref_type, override, cached, _ in CASES:
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override}
            use = evaluate(field(SOURCE, "CI_USE_CACHE"), context)
            save = evaluate(field(SOURCE, "CI_SAVE_CACHE"), context)
            context.update({"env.CI_USE_CACHE": str(use).lower(), "env.CI_SAVE_CACHE": str(save).lower()})
            for paths in ("true", "false"):
                with self.subTest(event=event, ref=ref, override=override, paths=paths):
                    context["steps.should-run.outputs.run"] = str(custody_should_run(SOURCE, context, paths)).lower()
                    args = []
                    if job_runs("custody-harness-smoke", context) and evaluate(field(step, "if"), context):
                        result = subprocess.run(["bash", "-eo", "pipefail", "-c", stub + script],
                                                env={**os.environ, "CI_USE_CACHE": str(use).lower(),
                                                     "CI_SAVE_CACHE": str(save).lower()},
                                                capture_output=True, check=True)
                        args = result.stdout.decode().split("\0")[:-1]
                    trusted_push = event == "push" and ref in SAVING_REFS
                    expected_run = event in ("merge_group", "workflow_dispatch") or trusted_push or \
                        (event == "pull_request" and paths == "true")
                    cache = (["--cache-from", "type=gha"] if cached else []) + \
                        (["--cache-to", "type=gha,mode=max"] if trusted_push else [])
                    self.assertEqual(args, base + cache + ["--load", "."] if expected_run else [])

    def test_github_runners_check_names_and_release_dependencies_stay_fixed(self):
        expected = {
            "source-gate": ("source-gate", "ubuntu-24.04"),
            "engine-llama-arm64": ("engine-llama-arm64", "ubuntu-24.04-arm"),
            "lint": ("lint", "ubuntu-24.04"),
            "test-elastos": ("test-elastos", "ubuntu-24.04"),
            "test-behaviour": ("test-behaviour", "ubuntu-24.04"),
            "test-capsules": ("test-capsules", "ubuntu-24.04"),
            "custody-harness-smoke": ("custody-harness-smoke", "ubuntu-24.04"),
            "source-home-linux": ("source-home-linux (${{ matrix.os }})", "${{ matrix.os }}"),
            "source-home-linux-arm64": ("source-home-linux (${{ matrix.check_name || matrix.os }})", "${{ matrix.os }}"),
            "source-home-macos": ("source-home-macos", "macos-14"),
            "cancel-on-failure": ("cancel-on-failure", "ubuntu-24.04"),
            "release": ("publish-github-release", "ubuntu-24.04"),
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
        needs = JOBS["release"].split("    needs:\n", 1)[1].split("    permissions:\n", 1)[0]
        self.assertEqual(re.findall(r"- ([\w-]+)", needs),
                         ["lint", "test-elastos", "test-behaviour", "test-capsules", "source-home-linux", "source-home-linux-arm64", "source-home-macos"])
        self.assertIn("python3 scripts/ci-release-policy-test.py", JOBS["source-gate"])

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
        for event, ref, ref_type, override, _, _ in CASES:
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

    def test_engine_cache_has_exact_recipe_key_and_develop_only_writers(self):
        restore, = [step for step in steps("engine-llama-arm64") if "actions/cache/restore@" in step]
        save, = [step for step in steps("engine-llama-arm64") if "actions/cache/save@" in step]
        self.assertEqual(field(restore, "key"), "${{ steps.engine-recipe.outputs.cache-key }}")
        self.assertEqual(field(save, "key"), "${{ steps.engine-cache.outputs.cache-primary-key }}")
        self.assertNotIn("restore-keys:", restore)
        for event, ref, ref_type, override, cached, _ in CASES:
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
            output = root / "output"
            env = {**os.environ, "GITHUB_OUTPUT": str(output),
                   "BUILD_CONTAINER_IMAGE": "pinned-image", "ENGINE_BUILD_TOOLS": "pinned-tools"}
            def key():
                output.write_text("")
                subprocess.run(["bash", "-euo", "pipefail", "-c", script],
                               cwd=root, env=env, check=True)
                return output.read_text()
            original = key()
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
        self.assertEqual(field(JOBS["source-home-linux"], "needs"), "source-gate")
        self.assertEqual(field(JOBS["source-home-linux-arm64"], "needs"),
                         "[source-gate, engine-llama-arm64]")
        download, = [step for step in steps("source-home-linux-arm64") if "actions/download-artifact@" in step]
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
            (root / "llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz").write_bytes(b"fixture engine")
            correct = hashlib.sha256(b"fixture engine").hexdigest()
            for checksum in (correct, "0" * 64, "", "untrusted", "md5:" + "0" * 64):
                result = subprocess.run(["bash", str(builder), "--verify-archive", directory],
                                        env={**os.environ, "CI_LLAMA_ARM64_SHA256": checksum},
                                        capture_output=True, text=True)
                self.assertEqual(result.returncode == 0, checksum == correct, result.stderr)

    def test_engine_elf_validator_refuses_wrong_architecture_libraries_and_symbols(self):
        source = (WORKFLOW.parents[2] / "scripts/build/build-llama-server-bundle.sh").read_text()
        validator = source.split('> "$out/elf-verification.txt" <<\'PY\'\n', 1)[1].split("\nPY", 1)[0]
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
                        exec(compile(validator, "engine ELF validator", "exec"), {})
                    else:
                        with self.assertRaises(AssertionError):
                            exec(compile(validator, "engine ELF validator", "exec"), {})


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

if __name__ == "__main__":
    unittest.main()
