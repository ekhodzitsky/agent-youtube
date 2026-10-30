"""Artifact privacy and identity regressions without publishing or real credentials."""
import importlib.util
import io
from pathlib import Path
import tarfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('prepare_release', ROOT / 'tools/prepare_release.py')
RELEASE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(RELEASE)


class ReleaseTests(unittest.TestCase):
    def test_archive_is_deterministic_and_contains_nested_forks(self):
        entries = {'Cargo.toml': b'package', 'LICENSE': b'grant',
                   'vendor/syom/src/lib.rs': b'fork',
                   'experiments/botguard/vendor/boa_engine/src/lib.rs': b'patched'}
        raw = io.BytesIO()
        with tarfile.open(fileobj=raw, mode='w') as archive:
            for name, data in entries.items():
                entry = tarfile.TarInfo('agent-youtube-1.0.0/' + name)
                entry.size = len(data)
                archive.addfile(entry, io.BytesIO(data))
        first = RELEASE.compress_source(raw.getvalue(), 'agent-youtube-1.0.0/')
        self.assertEqual(first, RELEASE.compress_source(raw.getvalue(), 'agent-youtube-1.0.0/'))
        with tarfile.open(fileobj=io.BytesIO(first), mode='r:gz') as archive:
            self.assertEqual({m.name.removeprefix('agent-youtube-1.0.0/') for m in archive}, set(entries))

    def test_private_paths_and_traversal_are_refused(self):
        for name in ['.git/config', 'backlog/tasks/private.md', 'x/.grok/config.toml',
                     'runs/audio.m4a', 'tools-local/runtime/token', 'x/.env', '../escape',
                     '/absolute', '.codex/config.toml', 'x/.serena/index']:
            with self.subTest(name=name):
                self.assertFalse(RELEASE.allowed_path(name))
        self.assertTrue(RELEASE.allowed_path('AGENTS.md'))
        self.assertTrue(RELEASE.allowed_path('tests/fixtures/tls/ca.der'))

    def test_unsafe_archive_members_are_rejected_before_compression(self):
        for name, kind in [('bundle/backlog/private.md', tarfile.REGTYPE),
                           ('bundle/link', tarfile.SYMTYPE),
                           ('bundle/../escape', tarfile.REGTYPE)]:
            with self.subTest(name=name):
                raw = io.BytesIO()
                with tarfile.open(fileobj=raw, mode='w') as archive:
                    entry = tarfile.TarInfo(name)
                    entry.type = kind
                    entry.linkname = '/private'
                    archive.addfile(entry)
                with self.assertRaises(ValueError):
                    RELEASE.compress_source(raw.getvalue(), 'bundle/')

    def test_binary_must_match_the_exact_clean_revision_and_version(self):
        producer = {'version': '1.0.0', 'revision': 'a' * 40, 'source_dirty': False}
        RELEASE.validate_identity(producer, '1.0.0', 'a' * 40)
        for key, value in [('version', '0.0.0'), ('revision', 'b' * 40),
                           ('source_dirty', True), ('source_dirty', None)]:
            with self.subTest(key=key):
                with self.assertRaises(ValueError):
                    RELEASE.validate_identity({**producer, key: value}, '1.0.0', 'a' * 40)
