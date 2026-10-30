import copy
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import access
import cli_resume


class AccessEvidence(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        (self.root / 'audio.m4a').write_bytes(b'prefix')
        identity = dict(video_id='of4B3aw7MXo', length=12)
        (self.root / 'audio.json').write_text(json.dumps(dict(identity=identity, progress=dict(bytes=6))))
        self.row = dict(verified_complete_audio=False, retryable=False, error_code='download_audio_track_changed',
                        metrics={'access_fault':dict(kind='track', injected=True, prefix_bytes=6,
                        prefix_sha256=hashlib.sha256(b'prefix').hexdigest(), identity=identity,
                        renewals=0, token_refreshes=0, changed_candidates=1)})

    def tearDown(self):
        self.temp.cleanup()

    def test_permanent_refusal_requires_preserved_prefix_and_identity(self):
        access.verify_fault(self.root, self.row, 'track')
        for key, value in [('injected', False), ('changed_candidates', 0), ('renewals', 1), ('prefix_sha256', 'bad'), ('identity', {})]:
            row = copy.deepcopy(self.row)
            row['metrics']['access_fault'][key] = value
            with self.assertRaises(ValueError):
                access.verify_fault(self.root, row, 'track')
        self.row['retryable'] = True
        with self.assertRaises(ValueError):
            access.verify_fault(self.root, self.row, 'track')

    def test_recovery_requires_measured_recovery_action(self):
        for kind, counter in [('403', 'renewals'), ('source-expiry', 'renewals'), ('token-expiry', 'token_refreshes')]:
            row = copy.deepcopy(self.row)
            row['metrics']['access_fault']['kind'] = kind
            with self.assertRaises(ValueError):
                access.verify_fault(self.root, row, kind)
            row['metrics']['access_fault'][counter] = 1
            access.verify_fault(self.root, row, kind)

    def test_cli_requires_one_json_and_jsonl_events(self):
        (self.root / 'case.stderr').write_text('{"schema_version":1,"event":"progress"}\n')
        process = dict(exit_code=0, timeout=False)
        cli_resume.validate_command(self.root, 'case', process, '{"schema_version":1,"ok":true}')
        for output in ['{}', '{"schema_version":1,"ok":false}', '{"schema_version":1,"ok":true}\n{}']:
            with self.assertRaises(ValueError):
                cli_resume.validate_command(self.root, 'case', process, output)
        (self.root / 'case.stderr').write_text('not json\n')
        with self.assertRaises(ValueError):
            cli_resume.validate_command(self.root, 'case', process, '{"schema_version":1,"ok":true}')
