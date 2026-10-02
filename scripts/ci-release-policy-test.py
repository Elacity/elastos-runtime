#!/usr/bin/env python3
"""Check CI release/cache decisions without builds, Docker, or publication."""
import os
import hashlib
import json
from pathlib import Path
import re
import runpy
import signal
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/ci.yml"
SOURCE = WORKFLOW.read_text()
# Read the fixed job/step indentation used here; actionlint checks YAML syntax.
def jobs(source):
    return dict(re.findall(r"(?ms)^  ([\w-]+):\n(.*?)(?=^  [\w-]+:\n|\Z)",
                           source.split("\njobs:\n", 1)[1]))


JOBS = jobs(SOURCE)
CACHE_RE = re.compile(
    r"uses: (?:Swatinem/rust-cache|actions/cache(?:/restore|/save)?)@"
    r"|^\s+cache(?:-from|-to)?:|type=gha", re.M)


def field(block, name):
    match = re.search(rf"(?m)^\s*{re.escape(name)}: ([^\n]+)$", block)
    if not match:
        raise AssertionError(f"missing {name}")
    return match[1]


def steps(job, workflow_jobs=JOBS):
    return re.split(r"(?m)^      - ", workflow_jobs[job].split("    steps:\n", 1)[1])[1:]


def evaluate(expression, context):
    # eval accepts only this repository's own ci.yml expressions, never external input.
    expression = expression.removeprefix("${{").removesuffix("}}").strip()
    expression = expression.replace("&&", " and ").replace("||", " or ")
    expression = re.sub(r"!(?!=)", " not ", expression)
    expression = re.sub(r"(?:github|inputs|env|steps|matrix)\.[\w.-]+",
                        lambda match: repr(context[match[0]]), expression)
    return bool(eval(expression, {"__builtins__": {}}, {"startsWith": str.startswith}))


# event, workflow ref, ref type, checkout override, caches, publication
CASES = [
    ("push", "refs/heads/main", "branch", "", True, False),
    ("push", "refs/heads/develop", "branch", "", True, False),
    ("push", "refs/heads/v-work", "branch", "", True, False),
    ("push", "refs/tags/v0.7.1", "tag", "", False, True),
    ("push", "refs/tags/candidate", "tag", "", False, False),
    ("pull_request", "refs/pull/1/merge", "branch", "", True, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "", True, False),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "v0.7.1", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/tags/v0.7.1", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "refs/heads/work", False, False),
    ("workflow_dispatch", "refs/heads/main", "branch", "a" * 40, False, False),
    ("workflow_dispatch", "refs/tags/v0.7.1", "tag", "main", False, False),
]
# Only pushes of merged code to these branches may write shared caches.
SAVING_REFS = {"refs/heads/develop", "refs/heads/main"}
NO_CACHE_HIT = {"steps.providers-cache.outputs.cache-hit": "false",
                "steps.engine-cache.outputs.cache-hit": "false"}


def validate_cache_guards(source):
    workflow_jobs = jobs(source)
    for job in workflow_jobs:
        for step in steps(job, workflow_jobs):
            matches = list(CACHE_RE.finditer(step))
            if not matches:
                continue
            # Buildx shell cache arguments live inside this explicit cache-only branch.
            shell_guards = list(re.finditer(
                r'(?ms)^\s*if \[\[ "\$\{CI_USE_CACHE\}" == "true" \]\]; then\n'
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
                           "env.CI_USE_CACHE": "false", "env.CI_SAVE_CACHE": "true",
                           "steps.should-run.outputs.run": "true", **NO_CACHE_HIT}
                if evaluate(guard, context):
                    raise AssertionError(f"cache guard permits uncached build in {job}")


def validate_arm_package_dependency(source):
    linux_steps = steps("source-home-linux", jobs(source))
    package, = [step for step in linux_steps
                if step.startswith("name: build and package release binaries\n")]
    verify, = [step for step in linux_steps
               if step.startswith("name: verify Jetson release compatibility\n")]
    for event, ref, ref_type, override, _, _ in CASES:
        for platform in ("ubuntu-24.04", "ubuntu-22.04-arm"):
            context = {"github.event_name": event, "github.ref": ref,
                       "github.ref_type": ref_type, "inputs.ref": override,
                       "matrix.os": platform}
            if evaluate(field(verify, "if"), context) and not evaluate(field(package, "if"), context):
                raise AssertionError("ARM compatibility check requires its packaged archive")


def validate_installed_greeting_limit(source):
    # Read the actual runs_create request's flat input literal; reject shape changes.
    request, = re.findall(
        r'(?ms)^  const created = await request\(assistant, "/api/provider/model/runs_create", \{\n'
        r'(.*?)^  \}\);', source)
    input_literal, = re.findall(r'input: \{([^{}]*)\}', request)
    limits = re.findall(r'(?:^|,)\s*max_output_tokens:\s*([0-9]+)\s*(?=,|$)', input_literal)
    if len(limits) != 1 or not 0 < int(limits[0]) <= 8:
        raise AssertionError("installed greeting requires 1 through 8 output tokens")


class ReleasePolicyTests(unittest.TestCase):
    def test_package_guards_preserve_arm_proof_and_skip_other_pr_archives(self):
        for job, platform in (("source-home-linux", "ubuntu-24.04"),
                              ("source-home-linux", "ubuntu-22.04-arm"),
                              ("source-home-macos", "macos-14")):
            package, = [step for step in steps(job)
                        if step.startswith("name: build and package release binaries\n")]
            for event, ref, ref_type, override, _, _ in CASES:
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override,
                           "matrix.os": platform}
                with self.subTest(job=job, platform=platform, event=event, ref=ref):
                    expected = event != "pull_request" or platform == "ubuntu-22.04-arm"
                    self.assertEqual(evaluate(field(package, "if"), context), expected)

    def test_arm_compatibility_always_has_its_archive(self):
        validate_arm_package_dependency(SOURCE)

    def test_skipping_the_arm_pr_archive_is_refused(self):
        broken = SOURCE.replace("github.event_name != 'pull_request' || matrix.os == 'ubuntu-22.04-arm'",
                                "github.event_name != 'pull_request'", 1)
        with self.assertRaisesRegex(AssertionError, "requires its packaged archive"):
            validate_arm_package_dependency(broken)

    def test_event_ref_matrix_controls_publication_and_every_cache_action(self):
        validate_cache_guards(SOURCE)
        caches = [(job, step) for job in JOBS for step in steps(job)
                  if CACHE_RE.search(step) and "type=gha" not in step]
        self.assertEqual(len(caches), 11)
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
                for should_run in (True, False):
                    context["steps.should-run.outputs.run"] = str(should_run).lower()
                    for job, step in caches:
                        expected = cached and (job != "custody-harness-smoke" or should_run)
                        if "actions/cache/save@" in step:
                            expected = expected and (event == "push" and ref == "refs/heads/develop"
                                                     if job == "engine-llama-arm64" else save)
                        self.assertEqual(evaluate(field(step, "if"), context), expected,
                                         f"cache guard in {job}")

    def test_pull_requests_never_save_shared_caches(self):
        for job in JOBS:
            for step in steps(job):
                if "Swatinem/rust-cache@" in step:
                    self.assertEqual(field(step, "save-if"), "${{ env.CI_SAVE_CACHE == 'true' }}",
                                     f"rust-cache in {job} must save only from develop or main")
                if "actions/cache@" in step and "kubo-cache" not in step:
                    self.fail(f"{job} uses actions/cache, which also saves from PR runs")

    def test_unguarded_cache_paths_are_rejected(self):
        additions = [
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
        for enabled in (True, False):
            with self.subTest(enabled=enabled):
                result = subprocess.run(["bash", "-eo", "pipefail", "-c", stub + script],
                                        env={**os.environ, "CI_USE_CACHE": str(enabled).lower()},
                                        capture_output=True, check=True)
                args = result.stdout.decode().split("\0")[:-1]
                cache = ["--cache-from", "type=gha", "--cache-to", "type=gha,mode=max"] if enabled else []
                self.assertEqual(args, base + cache + ["--load", "."])

    def test_github_runners_check_names_and_release_dependencies_stay_fixed(self):
        expected = {
            "source-gate": ("source-gate", "ubuntu-24.04"),
            "engine-llama-arm64": ("engine-llama-arm64", "ubuntu-24.04-arm"),
            "lint": ("lint", "ubuntu-24.04"),
            "test-elastos": ("test-elastos", "ubuntu-24.04"),
            "test-capsules": ("test-capsules", "ubuntu-24.04"),
            "custody-harness-smoke": ("custody-harness-smoke", "ubuntu-24.04"),
            "source-home-linux": ("source-home-linux (${{ matrix.check_name || matrix.os }})", "${{ matrix.os }}"),
            "source-home-macos": ("source-home-macos", "macos-14"),
            "release": ("publish-github-release", "ubuntu-24.04"),
        }
        self.assertEqual(set(JOBS), set(expected))
        for job, (name, runner) in expected.items():
            self.assertEqual(field(JOBS[job], "name"), name)
            self.assertEqual(field(JOBS[job], "runs-on"), runner)
        self.assertEqual(field(JOBS["source-home-linux"], "os"),
                         "[ubuntu-24.04, ubuntu-22.04-arm]")
        self.assertIn("          - os: ubuntu-22.04-arm\n"
                      "            check_name: ubuntu-24.04-arm\n",
                      JOBS["source-home-linux"])
        needs = JOBS["release"].split("    needs:\n", 1)[1].split("    permissions:\n", 1)[0]
        self.assertEqual(re.findall(r"- ([\w-]+)", needs),
                         ["lint", "test-elastos", "test-capsules", "source-home-linux", "source-home-macos"])
        self.assertIn("python3 scripts/ci-release-policy-test.py", JOBS["source-gate"])

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
        self.assertEqual(field(JOBS["source-home-linux"], "needs"),
                         "[source-gate, engine-llama-arm64]")
        download, = [step for step in steps("source-home-linux") if "actions/download-artifact@" in step]
        self.assertIn("name: llama-arm64-bundle", download)
        self.assertEqual(field(download, "if"), "matrix.os == 'ubuntu-22.04-arm'")
        consumer_verify, = [step for step in steps("source-home-linux") if "name: verify pinned ARM64 engine input" in step]
        self.assertEqual(field(consumer_verify, "if"), "matrix.os == 'ubuntu-22.04-arm'")
        self.assertNotIn("run-id:", download)
        self.assertNotIn("36501810782", SOURCE)
        verify = 'bash scripts/build/build-llama-server-bundle.sh --verify-archive "$RUNNER_TEMP/llama-arm64-bundle"'
        for job in ("engine-llama-arm64", "source-home-linux"):
            self.assertIn(verify, JOBS[job])
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


class InstalledJourneyTests(unittest.TestCase):
    def test_installed_greeting_has_a_small_explicit_output_allowance(self):
        source = (WORKFLOW.parents[2] / "scripts/ci-installed-home-journey.mjs").read_text()
        validate_installed_greeting_limit(source)

    def test_missing_zero_and_oversized_greeting_allowances_are_refused(self):
        source = (WORKFLOW.parents[2] / "scripts/ci-installed-home-journey.mjs").read_text()
        allowance = "max_output_tokens: 8"
        self.assertEqual(source.count(allowance), 1)
        for replacement in ("", "max_output_tokens: 0", "max_output_tokens: 9",
                            "max_output_tokens: 1024"):
            with self.subTest(allowance=replacement), self.assertRaises(AssertionError):
                validate_installed_greeting_limit(source.replace(allowance, replacement))

    def fixture(self, root, platform):
        home = root / "home"
        data = home / ("Library/Application Support/elastos" if platform == "macos"
                       else ".local/share/elastos")
        evidence = root / "evidence"
        (data / "bin").mkdir(parents=True)
        (data / "receipts").mkdir()
        evidence.mkdir()
        runtime = data / "bin/elastos"
        runtime.write_bytes(b"installed fixture Runtime")
        (data / "bin/model-provider").write_bytes(b"installed fixture model provider")
        receipt = {"source": {"commit": "c" * 40}, "runtime": {
            "installed_sha256": "sha256:" + hashlib.sha256(runtime.read_bytes()).hexdigest()}}
        (data / "receipts/source-home-installation.json").write_text(json.dumps(receipt))
        (data / "components.json").write_text("{}")
        return home, data, evidence, receipt

    def execute(self, home, data, evidence, available=20):
        journey = runpy.run_path(str(WORKFLOW.parents[2] / "scripts/ci-installed-journeys.py"))
        child = mock.Mock(pid=12345)
        child.poll.return_value = None
        response = mock.MagicMock()
        response.__enter__.return_value.status = 200

        def node_journey(*args, **kwargs):
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
                mock.patch.object(subprocess, "check_output", return_value="c" * 40 + "\n"), \
                mock.patch.object(subprocess, "Popen", return_value=child) as gateway, \
                mock.patch.object(subprocess, "run", side_effect=node_journey) as node, \
                mock.patch("urllib.request.urlopen", return_value=response), \
                mock.patch.object(os, "killpg") as stop, \
                mock.patch.dict(journey["run"].__globals__, {"disk_observation": lambda _: {
                    "capacity_bytes": 100, "available_bytes": available}}):
            try:
                journey["run"](home, data, evidence)
            finally:
                self.gateway, self.node, self.stop = gateway, node, stop
        return child

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
                self.stop.assert_called_once_with(child.pid, signal.SIGTERM)
                child.wait.assert_called_once_with(timeout=15)
                record = json.loads((evidence / "installed-journeys.json").read_text())
                self.assertEqual(record["installed_model_provider_sha256"],
                                 hashlib.sha256((data / "bin/model-provider").read_bytes()).hexdigest())

    def test_receipt_hash_and_disk_refuse_launch(self):
        for failure in ("receipt", "hash", "provider", "disk"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temp:
                home, data, evidence, receipt = self.fixture(Path(temp), "macos")
                if failure == "receipt":
                    receipt["source"]["commit"] = "d" * 40
                    (data / "receipts/source-home-installation.json").write_text(json.dumps(receipt))
                elif failure == "hash":
                    (data / "bin/elastos").write_bytes(b"changed fixture Runtime")
                elif failure == "provider":
                    (data / "bin/model-provider").unlink()
                expected = (RuntimeError if failure == "disk" else
                            FileNotFoundError if failure == "provider" else AssertionError)
                with self.assertRaises(expected):
                    self.execute(home, data, evidence, available=11 if failure == "disk" else 20)
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
            with self.assertRaises(FileExistsError):
                journey["fresh_fixture"](home, data, target_home)
            with mock.patch.dict(journey["fresh_fixture"].__globals__, {"disk_observation": lambda _: {
                    "capacity_bytes": 100, "available_bytes": 14}}):
                low_disk_home = Path(temp) / "low-disk"
                with self.assertRaisesRegex(RuntimeError, "15% free disk"):
                    journey["fresh_fixture"](home, data, low_disk_home)
                self.assertFalse(low_disk_home.exists())

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
                self.assertEqual(prepare.call_count, 3)
                for index, (_, run_data, _) in enumerate(calls):
                    self.assertEqual(prepare.call_args_list[index].args[0][2], str(run_data))
                self.assertFalse((data / "passkeys.json").exists())

    def test_mac_workflow_runs_three_fresh_journeys_and_uploads_their_receipts(self):
        self.assertIn("scripts/ci-installed-journeys.sh home-repeat", JOBS["source-home-macos"])
        source = (WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh").read_text()
        self.assertIn('"$EVIDENCE" --repeat 3', source)
        self.assertIn("source-home-journeys/**/*.json", JOBS["source-home-macos"])
        self.assertIn("source-home-journeys/**/*.png", JOBS["source-home-macos"])


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

    def test_spread_requires_three_same_candidate_installed_passes(self):
        row = {"candidate": "c" * 40, "source_tree": "t" * 40,
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

    def test_summary_requires_three_current_candidate_passes_and_preserves_failure(self):
        shell = (WORKFLOW.parents[2] / "scripts/ci-installed-journeys.sh").read_text()
        source = shell.split('python3 - "$EVIDENCE" "$DATA" <<\'PY\'\n', 1)[1].split('\nPY\n', 1)[0]
        for failure in (None, "missing", "candidate", "reply"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                data = root / "data"
                (data / "bin").mkdir(parents=True)
                runtime = data / "bin/elastos"
                runtime.write_bytes(b"installed fixture Runtime")
                row = {"candidate": "c" * 40, "source_tree": "d" * 40,
                       "installed_runtime_sha256": hashlib.sha256(runtime.read_bytes()).hexdigest(),
                       "installed_model_provider_sha256": "b" * 64,
                       "results": {name: "passed" for name in ["home_screenshots", "model_package_admission", "installed_runtime_reply"]},
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
                with mock.patch.object(sys, "argv", ["summary", str(root), str(data)]), \
                        mock.patch.object(sys, "platform", "darwin"), \
                        mock.patch.dict(os.environ, {"GITHUB_STEP_SUMMARY": str(root / "summary.md")}), \
                        mock.patch.object(subprocess, "check_output", return_value="c" * 40 + "\n"):
                    if failure:
                        with self.assertRaises(SystemExit):
                            exec(compile(source, "installed-summary", "exec"), {})
                    else:
                        exec(compile(source, "installed-summary", "exec"), {})
                result = json.loads((root / "core-summary.json").read_text())
                self.assertEqual(result["model_timing_spread"]["status"], "incomplete" if failure else "complete")
                if failure == "reply":
                    self.assertEqual(result["results"]["installed_runtime_reply"], "failed or not run")
                summary = (root / "summary.md").read_text()
                self.assertIn("OS file cache can warm", summary)
                if not failure:
                    self.assertIn("acknowledgement_total_ms", summary)
                    self.assertIn("Delta Applied count", summary)

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
