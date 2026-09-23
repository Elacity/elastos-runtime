#!/usr/bin/env python3
"""Check source-home Cargo argument expansion with the platform's Bash."""
import os
from pathlib import Path
import subprocess
import unittest


class CargoArgumentsTest(unittest.TestCase):
    def test_empty_and_populated_arrays(self):
        source = Path(__file__).with_name("setup-source-home.sh").read_text()
        start = source.index('SOURCE_HOME_CARGO_PROFILE="')
        end = source.index('\ncargo_built_binary_path()', start)
        initialization = source[start:end]
        commands = [
            line.strip()
            for line in source.replace("\\\n", " ").splitlines()
            if '"$CARGO_BIN" build' in line and 'SOURCE_HOME_CARGO_PROFILE_ARGS[@]' in line
        ]
        self.assertEqual(len(commands), 7, "Update coverage when Cargo call sites change")
        prelude = r'''
set -euo pipefail
ROOT='/tmp/source home argument test'
CARGO_BIN=capture
manifest="$ROOT/guest/Cargo.toml"
rust_target=aarch64-unknown-linux-musl
linker_env=CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER
linker='/tmp/test linker'
provider=test-provider
capture() { printf '%s\0' "$@"; }
env() { shift; "$@"; }
source_home_binary_manifest_path() { printf '%s/capsules/%s/Cargo.toml' "$ROOT" "$1"; }
'''
        extra = '--config profile.dev.package.elastos-server.debug=2 --locked'
        for profile in ("release", "dev"):
            for value in (None, "", "   ", extra):
                environment = os.environ.copy()
                environment["SOURCE_HOME_CARGO_PROFILE"] = profile
                environment.pop("SOURCE_HOME_CARGO_EXTRA_ARGS", None)
                if value is not None:
                    environment["SOURCE_HOME_CARGO_EXTRA_ARGS"] = value
                for command in commands:
                    with self.subTest(profile=profile, extra=value, command=command):
                        result = subprocess.run(
                            ["/bin/bash", "-c", prelude + initialization + "\n" + command],
                            env=environment, capture_output=True, timeout=5,
                        )
                        self.assertEqual(result.returncode, 0, result.stderr.decode())
                        arguments = result.stdout.decode().split("\0")[:-1]
                        self.assertNotIn("", arguments)
                        self.assertEqual(arguments.count("--release"), int(profile == "release"))
                        manifest = arguments[arguments.index("--manifest-path") + 1]
                        self.assertTrue(manifest.startswith("/tmp/source home argument test/"))
                        expected_extra = extra.split() if value == extra and "-p elastos-server" in command else []
                        actual_extra = arguments[arguments.index("--config"):] if "--config" in arguments else []
                        if expected_extra:
                            self.assertEqual(actual_extra[:len(expected_extra)], expected_extra)
                        else:
                            self.assertEqual(actual_extra, [])


if __name__ == "__main__":
    unittest.main()
