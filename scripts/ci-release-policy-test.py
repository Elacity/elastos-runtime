#!/usr/bin/env python3
"""Check CI release/cache decisions without builds, Docker, or publication."""
import os
from pathlib import Path
import re
import subprocess
import textwrap
import unittest


WORKFLOW = Path(__file__).resolve().parents[1] / ".github/workflows/ci.yml"
SOURCE = WORKFLOW.read_text()
# Read the fixed job/step indentation used here; actionlint checks YAML syntax.
JOBS = dict(re.findall(r"(?ms)^  ([\w-]+):\n(.*?)(?=^  [\w-]+:\n|\Z)",
                       SOURCE.split("\njobs:\n", 1)[1]))


def field(block, name):
    match = re.search(rf"(?m)^\s*{re.escape(name)}: ([^\n]+)$", block)
    if not match:
        raise AssertionError(f"missing {name}")
    return match[1]


def steps(job):
    return re.split(r"(?m)^      - ", JOBS[job].split("    steps:\n", 1)[1])[1:]


def evaluate(expression, context):
    expression = expression.removeprefix("${{").removesuffix("}}").strip()
    expression = expression.replace("&&", " and ").replace("||", " or ")
    expression = re.sub(r"!(?!=)", " not ", expression)
    expression = re.sub(r"(?:github|inputs|env|steps)\.[\w.-]+",
                        lambda match: repr(context[match[0]]), expression)
    return bool(eval(expression, {"__builtins__": {}}, {"startsWith": str.startswith}))


# event, workflow ref, ref type, checkout override, caches, publication
CASES = [
    ("push", "refs/heads/main", "branch", "", True, False),
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


class ReleasePolicyTests(unittest.TestCase):
    def test_event_ref_matrix_controls_publication_and_every_cache_action(self):
        caches = [(job, step) for job in JOBS for step in steps(job)
                  if re.search(r"uses: (Swatinem/rust-cache|actions/cache)@", step)]
        self.assertEqual(len(caches), 9)
        for event, ref, ref_type, override, cached, publish in CASES:
            with self.subTest(event=event, ref=ref, override=override):
                context = {"github.event_name": event, "github.ref": ref,
                           "github.ref_type": ref_type, "inputs.ref": override}
                use_cache = evaluate(field(SOURCE, "CI_USE_CACHE"), context)
                self.assertEqual(use_cache, cached)
                self.assertEqual(evaluate(field(JOBS["release"], "if"), context), publish)
                context["env.CI_USE_CACHE"] = str(use_cache).lower()
                for should_run in (True, False):
                    context["steps.should-run.outputs.run"] = str(should_run).lower()
                    for job, step in caches:
                        self.assertEqual(evaluate(field(step, "if"), context),
                                         cached and (job != "custody-harness-smoke" or should_run),
                                         f"cache guard in {job}")

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
            "lint": ("lint", "ubuntu-24.04"),
            "test-elastos": ("test-elastos", "ubuntu-24.04"),
            "test-capsules": ("test-capsules", "ubuntu-24.04"),
            "custody-harness-smoke": ("custody-harness-smoke", "ubuntu-24.04"),
            "source-home-linux": ("source-home-linux (${{ matrix.os }})", "${{ matrix.os }}"),
            "source-home-macos": ("source-home-macos", "macos-14"),
            "release": ("publish-github-release", "ubuntu-24.04"),
        }
        self.assertEqual(set(JOBS), set(expected))
        for job, (name, runner) in expected.items():
            self.assertEqual(field(JOBS[job], "name"), name)
            self.assertEqual(field(JOBS[job], "runs-on"), runner)
        self.assertEqual(field(JOBS["source-home-linux"], "os"),
                         "[ubuntu-24.04, ubuntu-24.04-arm]")
        needs = JOBS["release"].split("    needs:\n", 1)[1].split("    permissions:\n", 1)[0]
        self.assertEqual(re.findall(r"- ([\w-]+)", needs),
                         ["lint", "test-elastos", "test-capsules", "source-home-linux", "source-home-macos"])
        self.assertIn("python3 scripts/ci-release-policy-test.py", JOBS["source-gate"])


if __name__ == "__main__":
    unittest.main()
