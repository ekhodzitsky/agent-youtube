import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('check_models', ROOT / 'tools/check_models.py')
MODELS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODELS)


class ModelIntegrityTests(unittest.TestCase):
    def test_changed_missing_and_linked_models_are_refused_without_rewriting(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            path = root/'encoder'
            path.write_bytes(b'original')
            expected = {'encoder': hashlib.sha256(b'original').hexdigest()}
            self.assertEqual(MODELS.verify(root, expected), expected)
            path.write_bytes(b'changed')
            with self.assertRaises(ValueError):
                MODELS.verify(root, expected)
            self.assertEqual(path.read_bytes(), b'changed')
            path.unlink()
            with self.assertRaises(ValueError):
                MODELS.verify(root, expected)
            (root/'real').write_bytes(b'original')
            path.symlink_to(root/'real')
            with self.assertRaises(ValueError):
                MODELS.verify(root, expected)
