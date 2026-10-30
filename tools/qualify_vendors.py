#!/usr/bin/env python3
"""Test vendored codec runtime against pinned upstream tests in isolated copies.

Sibling repositories are read with git archive only; all builds and logs stay in
this repository's ignored runs/. No source checkout is modified.
"""
import argparse
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
REVISIONS = {
    "syom": "997bf56efda8700792dbc4b0136a36ad90eca328",
    "bandlimit": "f9bed39b532ab23e5030ded23c7d60e45d5afa09",
}
PREPARED_REVISIONS = {
    "syom": "c458e8a15d57612f4c2b92cbb0347d05c8f893c3",
    "bandlimit": "0559b94be9f55cdbf2bf7c87a600ee66c6b2e761",
}

# syom tests use the commit that captured the working-tree additions present
# in our runtime snapshot. The runtime itself is always the vendored source.


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-root", type=Path, default=ROOT.parent)
    parser.add_argument("--crate", choices=list(REVISIONS), action="append")
    parser.add_argument("--prepared-patches", action="store_true",
                        help="test prepared upstream patches instead of vendored overlays")
    args = parser.parse_args()
    (ROOT / "runs").mkdir(exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="vendor-qualification-", dir=ROOT / "runs"))
    results = []
    for name in args.crate or REVISIONS:
        revision = (PREPARED_REVISIONS if args.prepared_patches else REVISIONS)[name]
        record = {"crate": name, "upstream_revision": revision}
        target = run / name
        target.mkdir()
        try:
            archive = subprocess.run(
                ["git", "-C", str(args.upstream_root / name), "archive", revision],
                capture_output=True, check=True,
            ).stdout
            with tarfile.open(fileobj=io.BytesIO(archive)) as source:
                source.extractall(target, filter="data")
            if args.prepared_patches:
                patch = ROOT / "tools" / "upstream-codecs" / f"{name}.patch"
                command = ["git", "apply", "--directory", str(target.relative_to(ROOT)), str(patch)]
                subprocess.run(command[:2] + ["--check"] + command[2:], cwd=ROOT, check=True)
                subprocess.run(command, cwd=ROOT, check=True)
            else:
                shutil.copytree(ROOT / "vendor" / name / "src", target / "src", dirs_exist_ok=True)
            # Explicit current toolchain; do not install the snapshot's toolchain.
            command = ["cargo", "+stable", "test", "--locked", "--lib"]
            record["command"] = command
            with (run / f"{name}.log").open("wb") as log:
                process = subprocess.run(command, cwd=target, env=dict(os.environ, CARGO_TARGET_DIR=str(run / "targets" / name)), stdout=log, stderr=subprocess.STDOUT)
                if process.returncode == 0 and args.prepared_patches:
                    regression = command[:-1] + ["--test", "upstream_checkpoint"]
                    record["regression_command"] = regression
                    process = subprocess.run(regression, cwd=target, env=dict(os.environ, CARGO_TARGET_DIR=str(run / "targets" / name)), stdout=log, stderr=subprocess.STDOUT)
            record["exit_code"] = process.returncode
        except (OSError, subprocess.CalledProcessError, tarfile.TarError) as error:
            record["error"] = str(error)
            record["exit_code"] = 1
        results.append(record)
        (run / "report.json").write_text(json.dumps(results, indent=2) + "\n")
        print(json.dumps({"run": str(run), **record}), flush=True)
    return int(any(result["exit_code"] for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
