#!/usr/bin/env python3
"""The retired URL helper refuses every invocation without changing local data."""

import os
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
FETCH = ROOT / "scripts/fetch/fetch-model.sh"


def snapshot(root):
    return {
        str(path.relative_to(root)): (
            path.stat().st_mode,
            path.read_bytes() if path.is_file() else None,
        )
        for path in root.rglob("*")
    }


def main():
    with tempfile.TemporaryDirectory(prefix="elastos-retired-model-fetch-") as temp:
        root = Path(temp)
        data = root / "data"
        model = data / "models/stable.gguf"
        model.parent.mkdir(parents=True)
        model.write_bytes(b"existing model\n")
        before = snapshot(root)
        env = {
            **os.environ,
            "HOME": str(root),
            "XDG_DATA_HOME": str(root / "xdg"),
            "ELASTOS_DATA_DIR": str(data),
            "ELASTOS_COMPONENTS_MANIFEST": str(root / "absent-components.json"),
        }
        for args in [[], ["stable"], ["experimental"], ["--list"], ["--help"], ["unknown"]]:
            result = subprocess.run(
                [str(FETCH), *args], env=env, capture_output=True, text=True, timeout=5
            )
            assert result.returncode != 0, f"retired helper accepted {args}"
            assert not result.stdout, result.stdout
            assert len(result.stderr.splitlines()) == 1, result.stderr
            assert "docs/MODEL_PACKAGE_HANDOFF.md#receiver-steps" in result.stderr
            assert snapshot(root) == before, f"retired helper changed local data: {args}"
    print("PASS retired model fetch: six invocations refused; local data unchanged")


if __name__ == "__main__":
    main()
