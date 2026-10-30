import unittest
import json
import pathlib
import tempfile
import corpus

class CorpusTests(unittest.TestCase):
    def test_manifest_is_pinned_and_ids_are_unique(self):
        data = corpus.load_manifest(corpus.DEFAULT_MANIFEST)
        self.assertGreaterEqual(len(data['videos']), 15)
        self.assertLessEqual(len(data['videos']), 20)
        self.assertEqual(len({r['id'] for r in data['videos']}), len(data['videos']))
    def test_same_day_repeats_do_not_qualify_three_days(self):
        reports = [dict(started_at='2026-10-25T12:00:00+00:00', corpus_sha256='same', setup_complete=True, trials=[])] * 3
        result = corpus.aggregate(reports, expected_days=3)
        self.assertEqual(result['observed_dates'], ['2026-10-25'])
        self.assertFalse(result['calendar_complete'])
    def test_failures_skips_and_unknown_metrics_stay_distinct(self):
        result = corpus.aggregate([dict(started_at='2026-10-25T12:00:00+00:00', corpus_sha256='same', setup_complete=True,
            trials=[dict(tool='agent-youtube', video_id='of4B3aw7MXo', complete=False, reason='youtube_bot_check'),
                    dict(tool='yt-dlp', video_id='of4B3aw7MXo', complete=False, reason='skipped_server_cooldown')])])
        self.assertEqual(result['counts']['agent-youtube']['failed'], 1)
        self.assertEqual(result['counts']['yt-dlp']['skipped'], 1)
        self.assertIsNone(result['metrics']['agent-youtube']['media_wire_bytes'])
    def test_different_corpora_cannot_be_pooled(self):
        with self.assertRaises(ValueError):
            corpus.aggregate([dict(started_at='2026-10-25', corpus_sha256='a'), dict(started_at='2026-10-26', corpus_sha256='b')])

    def test_request_categories_are_summed_only_with_complete_instrumentation(self):
        categories = dict(watch=1, player=2, script=0, attestation=0, media=7, other=0)
        report = dict(started_at='2026-10-25', corpus_sha256='same', trials=[
            dict(tool='agent-youtube', complete=True, metrics=dict(downloader_http_requests=10, requests_by_endpoint=categories))
            for _ in range(2)])
        result = corpus.aggregate([report])
        self.assertEqual(result['metrics']['agent-youtube']['requests_by_endpoint'], {key:value*2 for key,value in categories.items()})
        del report['trials'][0]['metrics']['requests_by_endpoint']
        self.assertIsNone(corpus.aggregate([report])['metrics']['agent-youtube']['requests_by_endpoint'])

    def test_duplicate_and_invalid_ids_are_rejected(self):
        for invalid in ['duplicate', 'bad/id']:
            data = corpus.load_manifest(corpus.DEFAULT_MANIFEST)
            data['videos'][1]['id'] = data['videos'][0]['id'] if invalid == 'duplicate' else invalid
            with tempfile.TemporaryDirectory() as root:
                path = pathlib.Path(root) / 'manifest.json'
                path.write_text(json.dumps(data))
                with self.assertRaises(ValueError): corpus.load_manifest(path)

    def test_only_finished_attempted_distinct_days_qualify_calendar_coverage(self):
        reports = [dict(started_at=f'2026-10-{day}T12:00:00+03:00', corpus_sha256='same', finished=True,
            trials=[dict(tool='agent-youtube', complete=False, reason='youtube_bot_check')]) for day in [25,26,27]]
        self.assertTrue(corpus.aggregate(reports)['calendar_complete'])
        reports[2]['finished'] = False
        self.assertFalse(corpus.aggregate(reports)['calendar_complete'])

    def test_different_frozen_binaries_are_not_silently_pooled(self):
        with self.assertRaises(ValueError):
            corpus.aggregate([dict(started_at='2026-10-25', corpus_sha256='same', cohort={'binary':'a'}),
                              dict(started_at='2026-10-26', corpus_sha256='same', cohort={'binary':'b'})])

if __name__ == '__main__': unittest.main()
