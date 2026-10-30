#!/usr/bin/env python3
"""Build and measure the client-only RSA composition in an owned isolated target.

Never changes production dependencies, normal target binaries, other projects,
Git visibility or workflows. All raw media and evidence remain under runs/.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "experiments/qualification"))
import qualify as q
from series import save
from stall import cooldown_active


def digest(path):
    if not path.exists():
        return None
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


UPSTREAM_REV = "70f76c039e587192688af18a80d5d6435dedaf22"
PROTOTYPES = ["provider.rs", "rsa.rs", "capabilities.rs", "keyless.rs", "upstream.rs"]


def manifest_for(source, provider):
    original = 'rustls-rustcrypto = { version = "=0.0.2-alpha", path = "vendor/rustls-rustcrypto" }'
    current = 'rustls-rustcrypto = { git = "https://github.com/RustCrypto/rustls-rustcrypto", rev = "' + UPSTREAM_REV + '" }'
    if source.count(original) + source.count(current) != 1:
        raise ValueError("expected exactly one known production provider declaration")
    original = original if original in source else current
    replacement = ('rustls-pure = "=0.15.4"\nrsa = { version = "=0.9.10", default-features = false, features = ["sha2"] }\npastey = "=0.2.3"'
                   if provider == "composition" else
                   'rustls-rustcrypto = { git = "https://github.com/RustCrypto/rustls-rustcrypto", rev = "' + UPSTREAM_REV + '" }')
    if provider not in ("composition", "upstream"):
        raise ValueError("unknown provider candidate")
    return source.replace(original, replacement)


def result_envelope(log, command):
    results = [value for line in log.splitlines() if line.strip()
               if (value := json.loads(line)).get("command") == command]
    if len(results) != 1 or results[0].get("ok") is not True:
        raise ValueError("expected one successful command envelope")
    return results[0]["result"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--provider", choices=["composition", "upstream"], default="composition")
    parser.add_argument("--model-dir", type=Path, default=Path.home() / ".gigastt/models")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    os.umask(0o077)
    run = Path(tempfile.mkdtemp(prefix="rsa-tls-", dir=ROOT / "runs"))
    print(run, flush=True)
    candidate = run / "candidate"
    candidate.mkdir()
    normal = [ROOT / "target" / profile / "agent-youtube" for profile in ["debug", "release"]]
    before = {str(p.relative_to(ROOT)): digest(p) for p in normal}
    tracked = subprocess.check_output(["git", "ls-files", "-z"], cwd=ROOT).decode().split("\0")
    inputs = [p for p in tracked if p] + ["tools/rsa-tls/" + name for name in PROTOTYPES]
    for relative in sorted(set(inputs)):
        source = ROOT / relative
        if not source.resolve().is_relative_to(ROOT) or source.is_symlink():
            raise ValueError("qualification input must be an owned regular file")
        destination = candidate / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
    manifest = candidate / "Cargo.toml"
    manifest.write_text(manifest_for(manifest.read_text(), args.provider))
    capabilities = candidate / "tools/rsa-tls/capabilities.rs"
    if args.provider == "upstream":
        capabilities.write_text(capabilities.read_text().replace('path = "provider.rs"', 'path = "upstream.rs"'))
    with manifest.open("a") as output:
        output.write('\n[[example]]\nname = "tls_prototype_capabilities"\npath = "tools/rsa-tls/capabilities.rs"\n')
    tls = candidate / "src/download/tls.rs"
    module = json.dumps(str(candidate / "tools/rsa-tls" / ("provider.rs" if args.provider == "composition" else "upstream.rs")))
    tls.write_text(tls.read_text().replace("use crate::Result;",
        f"#[path = {module}]\nmod composition;\nuse crate::Result;", 1).replace(
        "rustls_rustcrypto::provider()", "composition::provider()"))
    build = candidate / "build.rs"
    build.write_text(build.read_text().replace('        "src",', '        "src",\n        "tools/rsa-tls",', 1))
    report = {"schema_version": 1, "provider": args.provider, "upstream_revision": UPSTREAM_REV if args.provider == "upstream" else None, "checks": [], "trials": [], "normal_before": before,
              "runner_sha256": digest(Path(__file__)), "qualifier_sha256": digest(Path(q.__file__)),
              "prototype_sha256": {name: digest(ROOT / "tools/rsa-tls" / name) for name in PROTOTYPES}}
    save(run / "report.json", report)
    env = dict(os.environ, CARGO_TARGET_DIR=str(run / "target"))

    def check(name, command, *, required=True):
        with (run / (name + ".log")).open("w") as log:
            result = subprocess.run(command, cwd=candidate, env=env, stdout=log, stderr=subprocess.STDOUT)
        report["checks"].append({"name": name, "exit_code": result.returncode})
        save(run / "report.json", report)
        print(name, result.returncode, flush=True)
        if required and result.returncode:
            raise RuntimeError(f"{name} failed; see owned local log")

    artifacts = run / "build.jsonl"
    with artifacts.open("w") as out, (run / "build.log").open("w") as err:
        result = subprocess.run(["cargo", "test", "--release", "--no-run", "--message-format=json"],
                                cwd=candidate, env=env, stdout=out, stderr=err)
    report["normal_after_build"] = {str(p.relative_to(ROOT)): digest(p) for p in normal}
    assert report["normal_after_build"] == before, "normal user executable changed"
    save(run / "report.json", report)
    if result.returncode:
        raise RuntimeError("isolated prototype build failed")
    binaries = {}
    for line in artifacts.read_text().splitlines():
        item = json.loads(line)
        if item.get("reason") != "compiler-artifact" or not item.get("executable"):
            continue
        path = Path(item["executable"]).resolve()
        if not path.is_relative_to(run / "target"):
            raise ValueError("candidate executable escaped its owned target")
        if item["target"]["kind"] == ["lib"] and item["profile"]["test"]:
            binaries["probe"] = path
        if item["target"]["kind"] == ["bin"] and not item["profile"]["test"]:
            binaries["worker"] = path
    assert set(binaries) == {"probe", "worker"}
    report["binaries"] = {name: digest(path) for name, path in binaries.items()}
    save(run / "report.json", report)
    check("tls-tests", [str(binaries["probe"]), "download::tls::tests"])
    check("protocols", ["cargo", "run", "--release", "--locked", "--example", "tls_prototype_capabilities"])
    line = next(line for line in (run / "protocols.log").read_text().splitlines() if line.startswith("TLS_CAPABILITIES "))
    report["protocols"] = json.loads(line.removeprefix("TLS_CAPABILITIES "))
    check("application-tests", ["cargo", "test", "--release", "--locked"])
    check("clippy", ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"])
    check("msrv", ["cargo", "+1.94.0", "check", "--locked", "--all-targets"])
    report["worker_identity"] = json.loads(subprocess.check_output([str(binaries["worker"]), "--version"], text=True))["result"]
    assert report["worker_identity"]["binary_sha256"] == report["binaries"]["worker"]
    assert digest(binaries["probe"]) == report["binaries"]["probe"]
    check("setup", [str(binaries["worker"]), "setup", "--runtime-dir", str(run / "runtime"), "--model-dir", str(args.model_dir.resolve())])
    report["setup"] = result_envelope((run / "setup.log").read_text(), "setup")
    assert digest(run / "runtime/gigastt-2.22.0") == "efb3c09b0ddf7872fe47bccc91f556ae728cf01035a94efe6f8e0c0795591cd1"
    check("doctor", [str(binaries["worker"]), "doctor", "--runtime-dir", str(run / "runtime"), "--work-dir", str(run / "doctor-job")])
    report["doctor"] = result_envelope((run / "doctor.log").read_text(), "doctor")
    cases = [("of4B3aw7MXo", None), ("rQMC7nKfAXE", None), ("bedsdFbhqhw", 65536)]
    report["planned_trials"] = len(cases)
    for index, (video, resume) in enumerate(cases):
        folder = run / f"trial-{index}"
        folder.mkdir()
        if cooldown_active():
            trial = {"complete": False, "reason": "skipped_server_cooldown"}
        else:
            trial = q.native(type("Args", (), {"native_binary": binaries["worker"], "native_client": "auto",
                             "native_resume_after_bytes": resume, "timeout": args.timeout})(), binaries["probe"], folder, video)
        trial.update(video_id=video, scenario="resume" if resume else "fresh")
        report["trials"].append(trial)
        save(run / "report.json", report)
        print(json.dumps({key: trial.get(key) for key in ["video_id", "complete", "reason"]}), flush=True)
        if trial.get("rate_limited") or cooldown_active():
            report["trials"].extend({"video_id": pending, "scenario": "resume" if prefix else "fresh",
                "complete": False, "reason": "skipped_server_cooldown"} for pending, prefix in cases[index + 1:])
            break
        time.sleep(5)
    report["normal_after_trials"] = {str(p.relative_to(ROOT)): digest(p) for p in normal}
    assert report["normal_after_trials"] == before
    report["finished"] = True
    save(run / "report.json", report)
    return int(any(check["exit_code"] for check in report["checks"]) or not all(t["complete"] for t in report["trials"]))


if __name__ == "__main__":
    raise SystemExit(main())
