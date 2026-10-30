#!/usr/bin/env python3
"""Read-only SHA-256 check for the documented GigaAM model release (not inference)."""
import argparse
import hashlib
import json
from pathlib import Path

EXPECTED = {
    'v3_e2e_rnnt_encoder_int8.onnx': '16805903b102d8b27bb0253de044f25e33a8a5bf07fce0c4da4ac1a5a7544091',
    'v3_e2e_rnnt_decoder.onnx': '7b0a16d67fd2cb37061decc93c69e364a9ab27afee3c57495d55b1c974cf7231',
    'v3_e2e_rnnt_joint.onnx': '602ff7017a93311aad34df1437c8d7f49911353c13d6eae7a6ee7b041339465c',
    'v3_e2e_rnnt_vocab.txt': '39abae20e692998290c574e606f11a9edef2902a1995463fcff63d1490cf22b7',
}


def verify(directory, expected=EXPECTED):
    result = {}
    for name, digest in expected.items():
        path = directory / name
        if path.is_symlink() or not path.is_file():
            raise ValueError('missing/nonregular model: ' + name)
        with path.open('rb') as stream:
            actual = hashlib.file_digest(stream, 'sha256').hexdigest()
        if actual != digest:
            raise ValueError('model SHA-256 mismatch: ' + name)
        result[name] = actual
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model-dir', type=Path, required=True)
    args = parser.parse_args()
    try:
        hashes = verify(args.model_dir)
    except (OSError, ValueError) as error:
        print(json.dumps({'ok': False, 'error': str(error)}))
        return 1
    print(json.dumps({'ok': True, 'release': 'models-v3-2026-10-09',
                      'sha256': hashes, 'inference': 'not_checked'}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
