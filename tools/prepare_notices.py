#!/usr/bin/env python3
"""Collect exact dependency license/notice texts for local distribution review.

Uses the locked Cargo graph, including platform/build packages. Does not approve
license choices, grant the project license, or bundle recognizer/model assets.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"], cwd=ROOT, text=True))
    records, texts = [], {}
    supplements = json.loads((ROOT / "tools/license-sources.json").read_text())
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        base = Path(package["manifest_path"]).parent
        if base == ROOT:
            continue
        candidates = set()
        for path in base.iterdir():
            if path.is_file() and path.name.lower().startswith(("license", "licence", "copying", "notice", "unlicense")):
                candidates.add(path)
            if path.is_dir() and path.name.lower() in ("licenses", "licences"):
                candidates.update(p for p in path.rglob("*") if p.is_file())
        # r-efi embeds its grants and copyright notices in AUTHORS.
        if package["name"] == "r-efi":
            candidates.add(base / "AUTHORS")
        if package.get("license_file"):
            candidates.add(base / package["license_file"])
        files = []
        for path in sorted(candidates):
            raw = path.read_bytes()
            digest = hashlib.sha256(raw).hexdigest()
            texts[digest] = raw.decode("utf-8")
            files.append({"path": path.relative_to(base).as_posix(), "sha256": digest})
        if not files:
            for item in supplements.get(package["name"] + "@" + package["version"], []):
                path = (ROOT / item["path"]).resolve()
                if not path.is_relative_to(ROOT):
                    raise ValueError("supplement must stay inside the repository")
                raw = path.read_bytes()
                digest = hashlib.sha256(raw).hexdigest()
                if digest != item["sha256"]:
                    raise ValueError("supplement license digest mismatch")
                texts[digest] = raw.decode("utf-8")
                files.append({"path": item["path"], "sha256": digest, "source": item["source"]})
        records.append({"name": package["name"], "version": package["version"],
                        "license": package["license"], "repository": package.get("repository"), "files": files})
    bundled = []
    for relative in ["vendor/botguard/LICENSE-RUSTYPIPE", "vendor/botguard/THIRD_PARTY_NOTICES.txt",
                     "experiments/botguard/vendor/LICENSE-EJS-UNLICENSE"]:
        raw = (ROOT / relative).read_bytes()
        digest = hashlib.sha256(raw).hexdigest()
        texts[digest] = raw.decode("utf-8")
        bundled.append({"path": relative, "sha256": digest})
    inventory = {"schema_version": 1, "packages": records, "bundled_assets": bundled,
                 "missing_texts": [p["name"] + "@" + p["version"] for p in records if not p["files"]],
                 "project_license_text_present": (ROOT / "LICENSE").is_file()}
    (args.output / "license-inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
    with (args.output / "THIRD_PARTY_NOTICES.txt").open("w") as output:
        output.write("Dependency text collection for review; not a distribution approval.\n\n")
        for package in records:
            output.write(f"{package['name']} {package['version']} — {package['license']}\n")
            for item in package["files"]:
                output.write(f"  {item['path']}: text SHA-256 {item['sha256']}\n")
        output.write("\nBundled JavaScript and algorithm notices:\n")
        for item in bundled:
            output.write(f"  {item['path']}: text SHA-256 {item['sha256']}\n")
        for digest, content in sorted(texts.items()):
            output.write(f"\n{'=' * 72}\nText SHA-256 {digest}\n\n{content}\n")
    print(json.dumps({"packages": len(records), "unique_texts": len(texts),
                      "missing_texts": inventory["missing_texts"],
                      "project_license_text_present": inventory["project_license_text_present"]}))
    return int(bool(inventory["missing_texts"]))


if __name__ == "__main__":
    raise SystemExit(main())
