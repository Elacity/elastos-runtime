#!/usr/bin/env python3
"""Package names and binary checks for scripts/package-release-binaries.sh."""

import importlib.util
import json
import os
import shutil
import stat
import struct
import subprocess
import tarfile
import tempfile
import textwrap
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/package-release-binaries.sh"
BASH = shutil.which("bash")
# Native Intel Mac: sysctl -i prints 0 or nothing for these two OIDs.
NATIVE_INTEL = ("0", "0")


def executable(path, text):
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IEXEC)


def write_shims(directory, system, machine, sysctl=NATIVE_INTEL):
    """uname reports system/machine. sysctl is a pair of answers, "fail", or None for missing."""
    directory = Path(directory)
    executable(directory / "uname",
               "#!/bin/sh\n"
               f"case \"$1\" in -s) printf '%s\\n' {system!r} ;; *) printf '%s\\n' {machine!r} ;; esac\n")
    if sysctl == "fail":
        executable(directory / "sysctl", "#!/bin/sh\necho 'sysctl: broken' >&2\nexit 1\n")
    elif sysctl is not None:
        translated, arm64 = sysctl
        executable(directory / "sysctl", textwrap.dedent(f"""\
            #!/bin/sh
            [ "$1" = -in ] || exit 2
            case "$2" in
                sysctl.proc_translated) printf '%s' {translated!r} ;;
                hw.optional.arm64) printf '%s' {arm64!r} ;;
                *) exit 2 ;;
            esac
            """))


def run_script(directory, *args, cwd=ROOT, script=SCRIPT, extra_env=None, only_shims=False):
    env = os.environ.copy()
    if only_shims:
        # Only the shims and dirname: a tool that is not shimmed is missing.
        os.symlink(shutil.which("dirname"), Path(directory) / "dirname")
        env["PATH"] = str(directory)
    else:
        env["PATH"] = str(directory) + os.pathsep + env["PATH"]
    env.update(extra_env or {})
    return subprocess.run([BASH, str(script), *args], cwd=cwd, env=env, text=True,
                          capture_output=True, timeout=60)


def package_name(system, machine, sysctl=NATIVE_INTEL, only_shims=False):
    with tempfile.TemporaryDirectory() as directory:
        write_shims(directory, system, machine, sysctl)
        return run_script(directory, "--print-platform", only_shims=only_shims)


def native_header(kind, machine):
    if kind == "elf":
        return (b"\x7fELF\x02\x01\x01" + bytes(9) + struct.pack("<HH", 3, machine)).ljust(64, b"\0")
    if kind == "macho":
        return (b"\xcf\xfa\xed\xfe" + struct.pack("<III", machine, 0, 2)).ljust(64, b"\0")
    # A universal (fat) Mach-O with arm64 and x86_64 slices.
    return (b"\xca\xfe\xba\xbe" + struct.pack(">IIIIII", 2, 0x100000C, 0, 0, 0, 0)).ljust(64, b"\0")


ELF_AMD64 = native_header("elf", 62)
ELF_ARM64 = native_header("elf", 183)
MACHO_ARM64 = native_header("macho", 0x100000C)
MACHO_AMD64 = native_header("macho", 0x1000007)
UNIVERSAL = native_header("fat", 0)


class PackageReleaseNameTest(unittest.TestCase):
    def test_native_hosts_use_the_release_platform_names(self):
        expected = {
            ("Linux", "x86_64"): "linux-amd64",
            ("Linux", "aarch64"): "linux-arm64",
            ("Linux", "arm64"): "linux-arm64",
            ("Darwin", "arm64"): "darwin-arm64",
            ("Darwin", "x86_64"): "darwin-amd64",
        }
        for host, name in expected.items():
            with self.subTest(host=host):
                result = package_name(*host)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), name)

    def test_native_intel_answers_name_darwin_amd64(self):
        # sysctl -i prints nothing for an OID that an Intel macOS lacks.
        for answers in (("0", "0"), ("", ""), ("0", ""), ("", "0")):
            with self.subTest(answers=answers):
                result = package_name("Darwin", "x86_64", answers)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), "darwin-amd64")

    def test_rosetta_shell_on_apple_silicon_is_not_named_darwin_amd64(self):
        for answers in (("1", "1"), ("1", ""), ("0", "1"), ("", "1")):
            with self.subTest(answers=answers):
                result = package_name("Darwin", "x86_64", answers)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertEqual(result.stdout, "")
                self.assertIn("Rosetta on an Apple silicon Mac", result.stderr)

    def test_intel_name_fails_closed_without_a_clear_sysctl_answer(self):
        cases = {"missing": None, "fails": "fail", "unknown value": ("2", "0"), "text": ("yes", "")}
        for case, sysctl in cases.items():
            with self.subTest(case=case):
                result = package_name("Darwin", "x86_64", sysctl, only_shims=True)
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertEqual(result.stdout, "")
                self.assertIn("sysctl", result.stderr)

    def test_intel_package_key_prints_and_packaging_refuses_the_tarball(self):
        platforms = json.loads((ROOT / "scripts/release-platforms.json").read_text())
        self.assertEqual(platforms["darwin-amd64"], "x86_64-darwin")
        # The release id behind that key is outside the signer's allowlist.
        spec = importlib.util.spec_from_file_location(
            "release_signer_platforms", ROOT / "scripts/release-signer.py")
        signer = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(signer)
        self.assertNotIn(platforms["darwin-amd64"], signer.RELEASE_PLATFORMS)
        printed = package_name("Darwin", "x86_64")
        self.assertEqual(printed.returncode, 0, printed.stderr)
        self.assertEqual(printed.stdout.strip(), "darwin-amd64")
        with tempfile.TemporaryDirectory() as directory:
            write_shims(directory, "Darwin", "x86_64")
            before = set(ROOT.glob("elastos-runtime-*.tar.gz"))
            result = run_script(directory, extra_env={"TMPDIR": directory})
            self.assertEqual(result.returncode, 1, result.stdout)
            self.assertIn("darwin-amd64 is not a signed release platform", result.stderr)
            self.assertEqual(set(ROOT.glob("elastos-runtime-*.tar.gz")), before)
            self.assertEqual(list(Path(directory).glob("elastos-runtime-*")), [])

    def test_print_platform_writes_nothing(self):
        with tempfile.TemporaryDirectory() as directory:
            write_shims(directory, "Darwin", "x86_64")
            scratch = Path(directory) / "tmp"
            scratch.mkdir()
            before = {path.name for path in ROOT.iterdir()}
            result = run_script(directory, "--print-platform", extra_env={"TMPDIR": str(scratch)})
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.strip(), "darwin-amd64")
            self.assertEqual({path.name for path in ROOT.iterdir()}, before)
            self.assertEqual(list(scratch.iterdir()), [])

    def test_other_darwin_hosts_stop(self):
        result = package_name("Darwin", "i386")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Unsupported release platform: Darwin-i386", result.stderr)


class PackageBinaryArchitectureTest(unittest.TestCase):
    """Packaging writes a tarball only when every binary matches its name."""

    def package(self, system, machine, headers):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        scripts = root / "scripts"
        scripts.mkdir()
        for name in ("package-release-binaries.sh", "release-platform-input.py", "release-platforms.json",
                     "release-signer.py", "components-release-integrity-check.py", "browser-host-release.py"):
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        shims = root / "shims"
        shims.mkdir()
        write_shims(shims, system, machine)
        names = sorted(headers)
        executable(shims / "cargo", "#!/bin/sh\nprintf '%s' " + repr(json.dumps({
            "workspace_members": ["member"],
            "packages": [{"id": "member", "targets": [{"name": name, "kind": ["bin"]} for name in names]}],
        })) + "\n")
        (root / "elastos").mkdir()
        (root / "elastos/Cargo.toml").touch()
        release = root / "elastos/target/release"
        release.mkdir(parents=True)
        for name, header in headers.items():
            (release / name).write_bytes(header + name.encode())
            (release / name).chmod(0o755)
        env = {"TMPDIR": str(root), "COPYFILE_DISABLE": "1", "CARGO_TARGET_DIR": str(root / "elastos/target")}
        result = run_script(shims, cwd=root, script=scripts / "package-release-binaries.sh", extra_env=env)
        return result, sorted(path.name for path in root.glob("elastos-runtime-*"))

    def test_matching_binaries_are_packaged(self):
        cases = {("Linux", "x86_64"): ("linux-amd64", ELF_AMD64),
                 ("Linux", "aarch64"): ("linux-arm64", ELF_ARM64),
                 ("Darwin", "arm64"): ("darwin-arm64", MACHO_ARM64)}
        for host, (key, header) in cases.items():
            with self.subTest(host=host):
                result, written = self.package(*host, {"elastos": header, "custody-provider": header})
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(written, [f"elastos-runtime-{key}.tar.gz", f"elastos-runtime-{key}.tar.gz.sha256"])
                tarball = Path(result.stdout.strip().splitlines()[-1].removeprefix("Wrote "))
                with tarfile.open(tarball) as archive:
                    self.assertEqual(sorted(Path(item.name).name for item in archive if item.isfile()),
                                     ["custody-provider", "elastos"])

    def test_binary_for_another_processor_or_universal_is_refused(self):
        cases = {
            "arm64 ELF named linux-amd64": (("Linux", "x86_64"), ELF_ARM64),
            "x86_64 ELF named linux-arm64": (("Linux", "aarch64"), ELF_AMD64),
            "x86_64 Mach-O named darwin-arm64": (("Darwin", "arm64"), MACHO_AMD64),
            "universal Mach-O named darwin-arm64": (("Darwin", "arm64"), UNIVERSAL),
            "Linux ELF named darwin-arm64": (("Darwin", "arm64"), ELF_ARM64),
            "shell script named linux-amd64": (("Linux", "x86_64"), b"#!/bin/sh\n".ljust(64, b"#")),
            "short file named linux-amd64": (("Linux", "x86_64"), b"\x7fELF"),
        }
        for case, (host, header) in cases.items():
            with self.subTest(case=case):
                good = {"Linux-x86_64": ELF_AMD64, "Linux-aarch64": ELF_ARM64, "Darwin-arm64": MACHO_ARM64}["-".join(host)]
                result, written = self.package(*host, {"elastos": good, "wrong-tool": header})
                self.assertEqual(result.returncode, 1, result.stdout)
                self.assertIn("Refusing to package", result.stderr)
                self.assertIn("wrong-tool", result.stderr)
                self.assertNotIn("elastos:", result.stderr)
                self.assertEqual(written, [])


if __name__ == "__main__":
    unittest.main()
