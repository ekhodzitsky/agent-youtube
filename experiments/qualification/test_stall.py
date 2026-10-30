import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest

import stall


class StallEvidence(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / 'audio.m4a').write_bytes(b'prefix')
        self.receipt = dict(identity=dict(video_id='a2hbJlPxj4M', length=12), progress=dict(bytes=6))
        (self.root / 'audio.json').write_text(json.dumps(self.receipt))
        self.result = dict(error_code='download_no_progress', retryable=True,
                           verified_complete_audio=False, metrics={'stall':dict(
                               prefix_bytes=6, prefix_sha256=hashlib.sha256(b'prefix').hexdigest(),
                               cycles=12, renewals=1, persistent=True, identity=self.receipt['identity'])})

    def tearDown(self):
        self.temp.cleanup()

    def test_persistent_failure_has_proven_prefix_and_bounded_renewal(self):
        self.assertEqual(stall.verify_stall(self.root, self.result, True)['prefix_bytes'], 6)

    def test_rejects_unexercised_or_corrupt_evidence(self):
        for key, value in [('cycles', 0), ('cycles', 100), ('renewals', 0), ('renewals', 2),
                           ('prefix_bytes', 0), ('prefix_bytes', 12), ('prefix_bytes', True),
                           ('prefix_sha256', 'wrong'), ('persistent', False), ('identity', {})]:
            with self.subTest(key=key, value=value):
                result = copy.deepcopy(self.result)
                result['metrics']['stall'][key] = value
                with self.assertRaises(ValueError):
                    stall.verify_stall(self.root, result, True)
        for key, value in [('retryable', False), ('error_code', 'youtube_rate_limited'),
                           ('verified_complete_audio', True)]:
            result = copy.deepcopy(self.result)
            result[key] = value
            with self.assertRaises(ValueError):
                stall.verify_stall(self.root, result, True)

    def test_failure_cannot_hide_uncommitted_bytes(self):
        (self.root / 'audio.m4a').write_bytes(b'prefixextra')
        with self.assertRaises(ValueError):
            stall.verify_stall(self.root, self.result, True)

    def test_recovered_prefix_must_still_match(self):
        evidence = self.result['metrics']['stall']
        evidence.update(persistent=False, cycles=8)
        (self.root / 'audio.m4a').write_bytes(b'prefixsuffix')
        self.assertEqual(stall.verify_stall(self.root, self.result, False), evidence)
        (self.root / 'audio.m4a').write_bytes(b'changed-data')
        with self.assertRaises(ValueError):
            stall.verify_stall(self.root, self.result, False)
