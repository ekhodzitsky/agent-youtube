"""Verify ignore boundaries without requiring the source archive to contain Git history."""
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class RepositoryIgnoreTests(unittest.TestCase):
    def ignored(self, paths):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            (root/'.gitignore').write_bytes((ROOT/'.gitignore').read_bytes())
            result = subprocess.run(
                ['git', '-c', 'core.excludesFile=/dev/null', 'check-ignore',
                 '--no-index', '--stdin', '-z'], cwd=root,
                input=('\0'.join(paths)+'\0').encode(), capture_output=True)
            self.assertIn(result.returncode, [0, 1])
            return set(result.stdout.decode().split('\0')) - {''}

    def test_nested_outputs_and_local_python_state_stay_out_of_git(self):
        paths = ['vendor/syom/target/debug/probe', 'examples/runs/job/transcript.json',
                 'examples/models/encoder.onnx', 'examples/tools-local/runtime/runtime.json',
                 '.venv/bin/python', 'venv/pyvenv.cfg', '.pytest_cache/nodeids',
                 '.mypy_cache/cache', '.ruff_cache/cache', 'coverage/lcov.info',
                 'profile.profraw', 'profile.profdata', 'x/backlog/private.md',
                 'x/.grok/config.toml', '.env.local', 'CLAUDE.md', 'x/CLAUDE.md']
        self.assertEqual(self.ignored(paths), set(paths))

    def test_source_forks_contracts_and_public_fixtures_remain_visible(self):
        paths = ['Cargo.lock', 'AGENTS.md', 'schemas/contract.schema.json',
                 'vendor/syom/src/lib.rs', 'vendor/rustls-rustcrypto-current/Cargo.toml',
                 'experiments/botguard/vendor/boa_engine/src/lib.rs',
                 'tests/fixtures/ps48.m4a', 'tests/fixtures/tls/ca.der',
                 'tests/fixtures/tls/rsa.pub', 'docs/release-061-results.json']
        self.assertEqual(self.ignored(paths), set())
