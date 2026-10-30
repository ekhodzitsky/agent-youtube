import datetime
import json
import os
import pathlib
import tempfile
import unittest
from unittest.mock import patch
import corpus
import qualify
import series


class SeriesSafety(unittest.TestCase):
    def test_cooldown_stops_later_trials_and_existing_series_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary, \
             patch.object(qualify, 'REPO', pathlib.Path(temporary)), \
             patch.dict(os.environ, {'XDG_CACHE_HOME': str(pathlib.Path(temporary)/'cache')}):
            root = pathlib.Path(temporary)/'runs'
            root.mkdir()
            artifact = root / 'artifact'
            artifact.write_bytes(b'not executable; tool calls must be mocked')
            date = datetime.datetime.now(datetime.timezone.utc).date().isoformat()
            keys = ['native_binary','probe','yt_dlp','node','ffmpeg','series_script','qualifier_script','corpus_module','manifest_file']
            config = dict(repository=str(qualify.REPO), output_root=str(root / 'campaign'), timezone='UTC', dates=[date],
                corpus=str(corpus.DEFAULT_MANIFEST), corpus_sha256=qualify.digest(corpus.DEFAULT_MANIFEST),
                seed=42, timeout=1, max_series_seconds=100, resume_videos=[],
                artifact_hashes={key:qualify.digest(artifact) for key in keys}, **{key:str(artifact) for key in keys})
            path = root / 'config.json'
            path.write_text(json.dumps(config))
            with patch.object(qualify,'reference',return_value=dict(complete=False,rate_limited=True,reason='youtube_rate_limited')) as reference, patch.object(qualify,'native') as native, patch('builtins.print'):
                result = series.run(path)
            self.assertEqual(result['state'], 'finished')
            reference.assert_called_once()
            native.assert_not_called()
            report = json.loads((root / 'campaign' / date / 'report.json').read_text())
            self.assertEqual(len(report['trials']), 30)
            self.assertTrue((root / 'campaign/pause.json').exists())
            self.assertEqual(sum(r.get('reason')=='skipped_server_cooldown' for r in report['trials']),29)
            self.assertEqual(series.run(path)['state'],'existing_series_preserved')
