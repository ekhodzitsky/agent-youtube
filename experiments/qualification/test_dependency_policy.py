"""Regression checks for selected dependency provenance and maintenance policy."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DependencyPolicyTests(unittest.TestCase):
    def test_syom_snapshot_matches_its_reviewed_provenance(self):
        import hashlib
        import json
        base = ROOT / 'vendor/syom'
        record = json.loads((base / 'upstream.json').read_text())
        self.assertEqual(record['release_status'], 'unreleased_source_candidate')
        self.assertEqual(record['runtime_dependencies'], 0)
        names = {item['path'] for item in record['files']}
        actual = {p.relative_to(base).as_posix() for p in base.rglob('*')
                  if p.is_file() and p.name != 'upstream.json'}
        self.assertEqual(names, actual)
        for item in record['files']:
            path = base / item['path']
            self.assertTrue(path.resolve().is_relative_to(base.resolve()))
            self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), item['bundled_sha256'])
            self.assertEqual(item['modified'], item['upstream_sha256'] != item['bundled_sha256'])
        patch = (base / record['patch']).resolve()
        self.assertTrue(patch.is_relative_to(ROOT))
        self.assertEqual(hashlib.sha256(patch.read_bytes()).hexdigest(), record['patch_sha256'])
        manifest = tomllib.loads((base / 'Cargo.toml').read_text())
        self.assertEqual(manifest['package']['version'], record['version'])
        self.assertFalse(manifest['dependencies'])

    def test_unmaintained_paste_does_not_return_to_the_locked_graph(self):
        packages = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
        self.assertNotIn('paste', {p['name'] for p in packages})
        self.assertIn(('rsa', '0.10.0-rc.19'),
                      {(p['name'], p['version']) for p in packages})
        self.assertIn(('crypto-bigint', '0.7.10'),
                      {(p['name'], p['version']) for p in packages})

    def test_provider_snapshot_rust_sources_match_recorded_upstream(self):
        import hashlib
        import json
        base = ROOT / 'vendor/rustls-rustcrypto-current'
        record = json.loads((base / 'upstream.json').read_text())
        sources = {p.relative_to(base).as_posix() for p in (base / 'src').rglob('*.rs')}
        self.assertEqual(sources, {p for p in record['upstream_sha256'] if p.startswith('src/')})
        for name in sources:
            self.assertEqual(hashlib.sha256((base / name).read_bytes()).hexdigest(),
                             record['upstream_sha256'][name], name)
        manifest = tomllib.loads((base / 'Cargo.toml').read_text())
        self.assertFalse(manifest['package']['publish'])
        self.assertEqual(manifest['dependencies']['paste']['package'], 'pastey')
        self.assertEqual(manifest['dependencies']['paste']['version'], '=0.2.3')
