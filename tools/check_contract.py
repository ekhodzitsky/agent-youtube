#!/usr/bin/env python3
"""Validate emitted documents against the local Draft 2020-12 contract.

Development-only dependency: install tools/check-requirements.txt. No network
schema references are needed; stdin is a JSON array of independent documents.
"""
import argparse
import json
from pathlib import Path
import sys

from jsonschema import Draft202012Validator

ROOT = Path(__file__).resolve().parents[1]


def validator():
    schema = json.loads((ROOT / "schemas/contract.schema.json").read_text())
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stdin", action="store_true", required=True)
    parser.parse_args()
    instances = json.load(sys.stdin)
    if not isinstance(instances, list):
        parser.error("stdin must contain an array of documents")
    check = validator()
    failures = []
    for index, instance in enumerate(instances):
        for error in check.iter_errors(instance):
            # Print schema paths, never the possibly private instance/message.
            failures.append({"document": index,
                             "schema_path": list(error.absolute_schema_path),
                             "instance_path": list(error.absolute_path)})
    print(json.dumps({"ok": not failures, "documents": len(instances),
                      "failures": failures}))
    return int(bool(failures))


if __name__ == "__main__":
    raise SystemExit(main())
