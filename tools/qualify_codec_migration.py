#!/usr/bin/env python3
"""Compare production and patched upstream in owned, ignored local snapshots.

No dependency switch or upstream writes. Requires Linux /usr/bin/time and
read-only sibling repositories containing PREPARED_REVISIONS. Real AAC/M4A
inputs can be added with repeated --source arguments; they are copied locally.
"""
import argparse
import array
import math
import sys
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

from qualify_vendors import ROOT, PREPARED_REVISIONS


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()



def pcm_difference(left, right):
    changed = samples = maximum = squared = 0
    with left.open("rb") as a, right.open("rb") as b:
        while block := a.read(65536):
            other = b.read(len(block))
            assert len(block) == len(other)
            x, y = array.array("h", block), array.array("h", other)
            if sys.byteorder != "little":
                x.byteswap()
                y.byteswap()
            for first, second in zip(x, y):
                delta = abs(first - second)
                changed += delta != 0
                samples += 1
                maximum = max(maximum, delta)
                squared += delta * delta
        assert not b.read(1)
    return {"changed_samples": changed, "samples": samples,
            "max_abs_pcm16": maximum, "rms_pcm16": math.sqrt(squared / samples) if samples else 0}

def archive(repo, revision, destination):
    data = subprocess.check_output(["git", "-C", str(repo), "archive", revision])
    with tarfile.open(fileobj=io.BytesIO(data)) as source:
        source.extractall(destination, filter="data")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream-root", type=Path, default=ROOT.parent)
    parser.add_argument("--source", type=Path, action="append", default=[])
    args = parser.parse_args()
    run = Path(tempfile.mkdtemp(prefix="codec-migration-", dir=ROOT / "runs"))
    print(run, flush=True)
    report = {"revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "upstream": PREPARED_REVISIONS, "cases": [], "checks": [],
              "patch_sha256": {name: digest(ROOT / "tools/upstream-codecs" / (name + ".patch")) for name in PREPARED_REVISIONS},
              "probe_sha256": digest(ROOT / "src/audio/qualification.rs"), "binaries": {}}

    def save():
        (run / "report.json").write_text(json.dumps(report, indent=2) + "\n")

    def command(name, cmd, cwd=ROOT, env=None, required=True):
        with (run / (name + ".log")).open("w") as log:
            result = subprocess.run(cmd, cwd=cwd, env=env, stdout=log, stderr=subprocess.STDOUT)
        report["checks"].append({"name": name, "exit_code": result.returncode})
        save()
        print(name, result.returncode, flush=True)
        if result.returncode and required:
            raise RuntimeError(f"{name} failed; see local log")

    candidate = run / "candidate"
    candidate.mkdir()
    archive(ROOT, "HEAD", candidate)
    # Include current qualification code, including not-yet-committed new files.
    for rel in ["src/audio.rs", "src/audio/qualification.rs", "tests/pipeline.rs"]:
        shutil.copy2(ROOT / rel, candidate / rel)
    for name, revision in PREPARED_REVISIONS.items():
        target = candidate / "vendor" / name
        shutil.rmtree(target)  # Owned extracted snapshot only.
        target.mkdir()
        archive(args.upstream_root / name, revision, target)
        patch = ROOT / "tools/upstream-codecs" / (name + ".patch")
        command("patch-" + name, ["git", "apply", "--directory", str(target.relative_to(ROOT)), str(patch)])
    manifest = candidate / "Cargo.toml"
    manifest.write_text(manifest.read_text().replace('syom = { version = "=0.7.1"', 'syom = { version = "=0.8.1"'))
    audio = candidate / "src/audio.rs"
    old = 'syom-0.7.1-bandlimit-0.1.7-ryf-0.7.3-mono-pcm16-v2'
    audio.write_text(audio.read_text().replace(old, 'candidate-syom-c458e8a-bandlimit-0559b94-patched-v1'))
    command("candidate-lock", ["cargo", "update", "--offline", "-p", "syom"], candidate)

    builds = {}
    for label, source in [("baseline", ROOT), ("candidate", candidate)]:
        # Never let experimental packages replace normal user-facing executables.
        env = dict(os.environ, CARGO_TARGET_DIR=str(run / "targets" / label))
        output = run / (label + "-build.jsonl")
        with output.open("w") as out, (run / (label + "-build.log")).open("w") as err:
            result = subprocess.run(["cargo", "test", "--locked", "--no-run", "--message-format=json"],
                                    cwd=source, env=env, stdout=out, stderr=err)
        if result.returncode:
            raise RuntimeError(f"{label} build failed")
        binaries = {}
        for line in output.read_text().splitlines():
            item = json.loads(line)
            if item.get("reason") == "compiler-artifact" and item.get("executable") and item["profile"]["test"]:
                name = "lib" if item["target"]["kind"] == ["lib"] else item["target"]["name"]
                dest = run / (label + "-" + name)
                shutil.copy2(item["executable"], dest)
                binaries[name] = dest
        builds[label] = binaries
        report["binaries"][label] = {name: digest(path) for name, path in binaries.items()}
        save()
    probe = "audio::qualification::migration_probe"
    inputs = run / "inputs"
    env = dict(os.environ, AGENT_YOUTUBE_CODEC_PROBE=str(inputs), AGENT_YOUTUBE_CODEC_MODE="generate")
    command("generate", [str(builds["baseline"]["lib"]), "--ignored", "--exact", probe], env=env)
    for index, source in enumerate(args.source):
        shutil.copyfile(source, inputs / f"real-{index}.m4a")
    for source in sorted(inputs.glob("*.m4a")):
        record = {"name": source.name, "input_sha256": digest(source), "builds": {}}
        for label, binaries in builds.items():
            work = run / label / source.stem
            metrics = {}
            for mode in ["full", "interrupt", "resume"]:
                env = dict(os.environ, AGENT_YOUTUBE_CODEC_PROBE=str(work), AGENT_YOUTUBE_CODEC_MODE=mode, AGENT_YOUTUBE_CODEC_SOURCE=str(source))
                rss = run / f"{label}-{source.stem}-{mode}.rss"
                command(f"{label}-{source.stem}-{mode}", ["/usr/bin/time", "-f", "%M", "-o", str(rss),
                    str(binaries["lib"]), "--ignored", "--exact", probe], env=env)
                value = json.loads((work / (mode + ".json")).read_text())
                value["max_rss_kib"] = int(rss.read_text().strip())
                metrics[mode] = value
            assert metrics["full"]["sha256"] == metrics["resume"]["sha256"], (label, source.name)
            record["builds"][label] = metrics
        left, right = [record["builds"][label]["full"] for label in builds]
        record["same_pcm"] = left["sha256"] == right["sha256"]
        record["same_frames"] = left["frames"] == right["frames"]
        assert record["same_frames"], source.name
        record["difference"] = pcm_difference(run / "baseline" / source.stem / "full.pcm",
                                               run / "candidate" / source.stem / "full.pcm")
        report["cases"].append(record)
        save()
    env = dict(os.environ, AGENT_YOUTUBE_CANDIDATE_PIPELINE=str(builds["candidate"]["pipeline"]))
    command("old-job-refusal", [str(builds["baseline"]["pipeline"]), "--ignored", "--exact",
            "candidate_backend_refuses_old_job_without_mutation", "--nocapture"], env=env, required=False)
    # Full application integration tests use the same fake recognizer as normal CI.
    for target in ["lib", "pipeline"]:
        command("candidate-" + target + "-tests", [str(builds["candidate"][target])])
    report["ready_for_migration"] = False
    report["reason"] = "Qualification only; published checkpoint APIs and cross-version policy remain required"
    save()
    print("Completed", run, flush=True)
    return int(any(check["exit_code"] for check in report["checks"]))


if __name__ == "__main__":
    raise SystemExit(main())
