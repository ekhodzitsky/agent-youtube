#!/usr/bin/env python3
"""Check portable Markdown links against the tracked tree, never ignored files."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit
from prepare_release import allowed_path

ROOT = Path(__file__).resolve().parents[1]


def inventory(working_tree=False, source_tree=False):
    if source_tree:
        files = set()
        for directory, dirs, names in os.walk(ROOT, followlinks=False):
            base = Path(directory)
            dirs[:] = [name for name in dirs
                       if allowed_path((base/name).relative_to(ROOT).as_posix())
                       and not (base/name).is_symlink()]
            files.update((base/name).relative_to(ROOT).as_posix() for name in names
                         if allowed_path((base/name).relative_to(ROOT).as_posix())
                         and not (base/name).is_symlink())
        return files
    top = subprocess.run(['git', 'rev-parse', '--show-toplevel'], cwd=ROOT,
                         capture_output=True, text=True)
    if top.returncode or Path(top.stdout.strip()).resolve() != ROOT.resolve():
        raise ValueError('no matching Git root; use --source-tree for an extracted archive')
    command = ['git', 'ls-files', '-z', '--cached']
    if working_tree:
        command += ['--others', '--exclude-standard']
    return set(subprocess.check_output(command, cwd=ROOT).decode().split('\0'))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--working-tree", action="store_true")
    mode.add_argument("--source-tree", action="store_true",
                      help="check an extracted source tree without admitting private-state paths")
    args = parser.parse_args()
    try:
        files = inventory(args.working_tree, args.source_tree)
    except ValueError as error:
        parser.error(str(error))
    failures = []
    documents = sorted(p for p in files if p == "README.md" or p.startswith("docs/") and p.endswith(".md"))
    for name in documents:
        text = (ROOT / name).read_text()
        text = re.sub(r"```.*?```", "", text, flags=re.S)
        for target in re.findall(r"\]\((<[^>]+>|[^\s)]+)\)", text):
            target = target.strip("<>")
            parsed = urlsplit(target)
            if parsed.scheme or parsed.netloc or not parsed.path:
                continue
            path = (ROOT / name).parent / unquote(parsed.path)
            try:
                relative = path.resolve().relative_to(ROOT).as_posix()
            except ValueError:
                relative = "<outside repository>"
            if relative not in files:
                failures.append({"source": name, "target": target})
    print(json.dumps({"ok": not failures, "documents": len(documents), "failures": failures}, indent=2))
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
