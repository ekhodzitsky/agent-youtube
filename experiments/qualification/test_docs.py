from pathlib import Path
import sys
import subprocess
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools'))
import check_docs


class DocumentationInventoryTests(unittest.TestCase):
    def test_nested_snapshot_cannot_report_a_zero_document_success(self):
        with tempfile.TemporaryDirectory() as temp:
            subprocess.run(['git', 'init', '-q', temp], check=True)
            root = Path(temp)/'snapshot'
            root.mkdir()
            (root/'README.md').write_text('[private](backlog/private.md)')
            with patch.object(check_docs, 'ROOT', root):
                with self.assertRaises(ValueError):
                    check_docs.inventory()

    def test_explicit_source_tree_does_not_admit_private_targets(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root/'docs').mkdir()
            (root/'docs/guide.md').write_text('guide')
            (root/'backlog').mkdir()
            (root/'backlog/private.md').write_text('private')
            with patch.object(check_docs, 'ROOT', root):
                files = check_docs.inventory(source_tree=True)
            self.assertIn('docs/guide.md', files)
            self.assertNotIn('backlog/private.md', files)
