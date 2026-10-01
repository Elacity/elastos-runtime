#!/usr/bin/env python3
"""Keep the standalone installer's signed version gate aligned with release policy."""
from pathlib import Path
import re
import subprocess
import unittest


ROOT = Path(__file__).resolve().parents[1]


def expressions(source, prefix=""):
    return "\n".join(re.search(rf"(?m)^{prefix}{name}=.*$", source)[0]
                     for name in ("core", "meta", "preferred"))


class InstallVersionPolicyTests(unittest.TestCase):
    def test_installer_expressions_match_release_policy(self):
        installer = (ROOT / "scripts/install.sh").read_text()
        policy = (ROOT / "scripts/check-versioning.sh").read_text()
        self.assertEqual(expressions(installer, "release_version_").replace("release_version_", ""),
                         expressions(policy))
        self.assertIn('[[ "$RELEASE_VERSION" =~ $release_version_preferred ]]', installer)
        self.assertLess(installer.index('[[ "$RELEASE_VERSION" =~ $release_version_preferred ]]'),
                        installer.index('INSTALLED_VERSION="${RELEASE_VERSION}"'))

    def test_standalone_gate_accepts_only_release_versions(self):
        installer = (ROOT / "scripts/install.sh").read_text()
        script = expressions(installer, "release_version_") + '\n[[ "$1" =~ $release_version_preferred ]]'
        for version, accepted in [
            ("0.7.3", True), ("0.7.3-rc.1", True), ("0.7.3-beta.0+build.1", True),
            ("", False), ("unknown", False), ("0.7", False), ("0.7.03", False),
            ("0.7.3-rc1", False), ("0.7.3-rc.01", False), ("0.7.3-other.1", False),
        ]:
            with self.subTest(version=version):
                result = subprocess.run(["bash", "-c", script, "version-test", version],
                                        capture_output=True)
                policy = subprocess.run(["bash", str(ROOT / "scripts/check-versioning.sh"), version],
                                        capture_output=True)
                self.assertEqual(result.returncode == 0, accepted)
                self.assertEqual(result.returncode == 0, policy.returncode == 0)


if __name__ == "__main__":
    unittest.main()
