"""Qualification must never silently test the production graph as a candidate."""
import importlib.util
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('tls_candidates', Path(__file__).resolve().parents[2] / 'tools/qualify_rsa_tls.py')
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class CandidateManifestTests(unittest.TestCase):
    declaration = 'rustls-rustcrypto = { version = "=0.0.2-alpha", path = "vendor/rustls-rustcrypto" }'

    def test_unknown_or_duplicated_production_provider_is_refused(self):
        for source in ['', self.declaration + '\n' + self.declaration]:
            with self.assertRaises(ValueError):
                module.manifest_for(source, 'composition')

    def test_progress_lines_do_not_replace_the_success_envelope(self):
        log = '{"event":"progress","schema_version":1}\n{"command":"doctor","ok":true,"result":{"preflight_passed":true}}\n'
        self.assertEqual(module.result_envelope(log, 'doctor'), {'preflight_passed': True})
        with self.assertRaises(ValueError):
            module.result_envelope(log + log, 'doctor')
        with self.assertRaises(ValueError):
            module.result_envelope('{"command":"doctor","ok":false}', 'doctor')

    def test_candidates_are_distinct_and_upstream_is_revision_pinned(self):
        composition = module.manifest_for(self.declaration, 'composition')
        upstream = module.manifest_for(self.declaration, 'upstream')
        self.assertIn('rustls-pure = "=0.15.4"', composition)
        self.assertNotIn('git =', composition)
        self.assertIn(module.UPSTREAM_REV, upstream)
        self.assertNotIn('rustls-pure', upstream)
        self.assertEqual(module.manifest_for(upstream, 'composition'), composition)
        self.assertEqual(module.manifest_for(upstream, 'upstream'), upstream)
        with self.assertRaises(ValueError):
            module.manifest_for(self.declaration, 'unrecognized')
