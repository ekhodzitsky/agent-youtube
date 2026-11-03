"""Regression checks for selected dependency provenance and maintenance policy."""
from pathlib import Path
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[2]


class DependencyPolicyTests(unittest.TestCase):
    def test_syom_uses_immutable_upstream_without_a_local_fork(self):
        manifest = tomllib.loads((ROOT / 'Cargo.toml').read_text())
        dep = manifest['dependencies']['syom']
        self.assertNotIn('path', dep)
        self.assertEqual(dep['git'], 'https://github.com/ekhodzitsky/syom')
        self.assertEqual(dep['rev'], '1383825e013f95ee565eb094122f3ec6fa2f7193')
        self.assertEqual(dep['version'], '=0.10.0')
        self.assertFalse((ROOT / 'vendor/syom').exists())
        packages = tomllib.loads((ROOT / 'Cargo.lock').read_text())['package']
        syom = next(p for p in packages if p['name'] == 'syom')
        self.assertEqual(syom['version'], '0.10.0')
        self.assertEqual(syom['source'], 'git+' + dep['git'] + '?rev=' + dep['rev'] + '#' + dep['rev'])

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
