import hashlib
import json
import pathlib
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace
import wave

import qualify


class Completeness(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temp.name)
        (self.root / 'audio.m4a').write_bytes(b'audio fixture')
        with wave.open(str(self.root / 'audio.wav'), 'wb') as wav:
            wav.setparams((1, 2, 16000, 0, 'NONE', 'not compressed'))
            wav.writeframes(b'\0' * 32000)
        self.row = dict(video_id='of4B3aw7MXo', verified_complete_audio=True,
                        declared_seconds=1, decoded_seconds=1, declared_bytes=13,
                        bytes=13, sha256=hashlib.sha256(b'audio fixture').hexdigest())
        self.receipt = dict(identity=dict(video_id='of4B3aw7MXo', length=13))
        (self.root / 'audio.json').write_text(json.dumps(self.receipt))

    def tearDown(self):
        self.temp.cleanup()

    def verify(self):
        return qualify.verify_native(self.root, self.row, 'of4B3aw7MXo')

    def test_native_timeout_does_not_read_missing_or_stale_completion(self):
        args = SimpleNamespace(timeout=1, native_resume_after_bytes=None)
        process = dict(exit_code=-15, timeout=True, rate_limited=False, wall_seconds=1.01)
        with patch.object(qualify, 'run', return_value=(process, '')), patch.object(qualify, 'read_json', side_effect=FileNotFoundError('missing probe result')) as read:
            result = qualify.native(args, pathlib.Path('probe'), self.root, self.row['video_id'])
        self.assertFalse(result['complete'])
        self.assertEqual(result['reason'], 'native_trial_timeout')
        self.assertTrue(result['timeout'])
        read.assert_not_called()

    def test_full_source_and_pcm(self):
        self.assertEqual(self.verify()['decoded_seconds'], 1)

    def test_resume_reports_final_cause_and_both_process_times(self):
        media = self.root / 'media'
        media.mkdir()
        (media / 'audio.m4a').write_bytes(b'audio fixture')
        initial = dict(self.row, cancelled=True, verified_complete_audio=False, error_code='interrupted', retry_after_seconds=1, metrics={'downloader_http_requests':2,'verified_replayed_audio_bytes':0})
        final = dict(self.row, metrics={'downloader_http_requests':3,'verified_replayed_audio_bytes':2})
        initial['metrics']['requests_by_endpoint'] = dict(watch=1, player=0, script=0, attestation=0, media=1, other=0)
        final['metrics']['requests_by_endpoint'] = dict(watch=0, player=0, script=0, attestation=0, media=3, other=0)
        for key in ['injected_dns_failures', 'cdn_fallback_attempts', 'cdn_fallback_selections']:
            initial['metrics'][key], final['metrics'][key] = 1, 0
        receipt = dict(self.receipt, progress={'bytes':2,'digest':qualify.digest(media / 'audio.m4a',2)})
        def read(path):
            return {'result.json':initial,'result.resume.json':final,'audio.json':receipt}[path.name]
        def run(*args):
            return dict(exit_code=0, timeout=False, rate_limited=False, wall_seconds=2), ''
        args = SimpleNamespace(native_client='auto', native_resume_after_bytes=2, timeout=10)
        with patch.object(qualify, 'run', run), patch.object(qualify, 'read_json', read), patch.object(qualify, 'verify_native', lambda *a: {'sha256':self.row['sha256']}):
            result = qualify.native(args, pathlib.Path('probe'), self.root, self.row['video_id'])
        self.assertTrue(result['complete'])
        self.assertIsNone(result['error_code'])
        self.assertIsNone(result['http_status'])
        self.assertIsNone(result['retry_after_seconds'])
        self.assertEqual(result['initial_error_code'], 'interrupted')
        self.assertEqual(result['wall_seconds'], 4)
        self.assertEqual(result['metrics']['downloader_http_requests'], 5)
        for key in ['injected_dns_failures', 'cdn_fallback_attempts', 'cdn_fallback_selections']:
            self.assertEqual(result['metrics'][key], 1)
        self.assertEqual(result['metrics']['requests_by_endpoint'], dict(watch=1, player=0, script=0, attestation=0, media=4, other=0))
        self.assertIn('exclude interpreter-worker HTTP', result['metrics']['coverage'])

    def test_resumed_failure_retains_final_cooldown_diagnostics(self):
        media = self.root / 'media'
        media.mkdir()
        (media / 'audio.m4a').write_bytes(b'audio fixture')
        initial = dict(self.row, cancelled=True, verified_complete_audio=False, error_code='interrupted')
        final = dict(verified_complete_audio=False, error_code='youtube_rate_limited',
                     http_status=302, retry_after_seconds=120, retry_after='private-header')
        receipt = dict(self.receipt, progress={'bytes':2,'digest':qualify.digest(media / 'audio.m4a',2)})
        def read(path):
            return {'result.json':initial,'result.resume.json':final,'audio.json':receipt}[path.name]
        args = SimpleNamespace(native_client='auto', native_resume_after_bytes=2, timeout=10)
        process = dict(exit_code=0, timeout=False, rate_limited=False, wall_seconds=2)
        with patch.object(qualify, 'run', side_effect=lambda *args: (dict(process), '')), patch.object(qualify, 'read_json', read):
            result = qualify.native(args, pathlib.Path('probe'), self.root, self.row['video_id'])
        self.assertFalse(result['complete'])
        self.assertEqual(result['reason'], 'youtube_rate_limited')
        self.assertEqual(result['http_status'], 302)
        self.assertEqual(result['retry_after_seconds'], 120)
        self.assertEqual(result['initial_error_code'], 'interrupted')
        self.assertNotIn('private-', json.dumps(result))
        self.assertIsNone(result['metrics']['requests_by_endpoint'])

    def test_request_category_aggregation_rejects_unknown_or_inconsistent_counts(self):
        categories = dict(watch=1, player=0, script=0, attestation=0, media=2, other=0)
        metric = dict(downloader_http_requests=3, requests_by_endpoint=categories)
        self.assertEqual(qualify.sum_request_categories([metric]), categories)
        self.assertIsNone(qualify.sum_request_categories([]))
        self.assertIsNone(qualify.sum_request_categories([None]))
        self.assertIsNone(qualify.sum_request_categories([metric, {}]))
        for counts in [None, {}, dict(categories, watch=True), dict(categories, media=-1), dict(categories, other=1)]:
            with self.subTest(counts=counts):
                self.assertIsNone(qualify.sum_request_categories([dict(metric, requests_by_endpoint=counts)]))
        with_private_key = dict(metric, requests_by_endpoint=dict(categories, private_token='private-secret'))
        self.assertIsNone(qualify.sum_request_categories([with_private_key]))
        future_category = dict(metric, requests_by_endpoint=dict(categories, future=0))
        self.assertIsNone(qualify.sum_request_categories([future_category]))

    def test_success_flag_does_not_hide_missing_pcm(self):
        (self.root / 'audio.wav').unlink()
        with self.assertRaises((ValueError, OSError)):
            self.verify()

    def test_native_failure_preserves_only_numeric_retry_diagnostics(self):
        args = SimpleNamespace(native_client='auto', native_resume_after_bytes=None, timeout=10)
        for status, seconds, expected_status, expected_seconds in [
            (302, 120, 302, 120), (429, None, 429, None),
            (None, 60, None, 60), ('private-status', 'private-header', None, None),
        ]:
            with self.subTest(status=status, seconds=seconds):
                row = dict(error_code='youtube_rate_limited', http_status=status,
                           retry_after_seconds=seconds, retry_after='private-header',
                           details={'url':'private-url'})
                with patch.object(qualify, 'run', return_value=(dict(exit_code=0, timeout=False, rate_limited=False), '')), patch.object(qualify, 'read_json', return_value=row):
                    result = qualify.native(args, pathlib.Path('probe'), self.root, self.row['video_id'])
                self.assertFalse(result['complete'])
                self.assertEqual(result['reason'], 'youtube_rate_limited')
                self.assertEqual(result['http_status'], expected_status)
                self.assertEqual(result['retry_after_seconds'], expected_seconds)
                self.assertNotIn('private-', json.dumps(result))

    def test_pcm_header_does_not_hide_truncated_payload(self):
        wav = self.root / 'audio.wav'
        wav.write_bytes(wav.read_bytes()[:-2])
        with self.assertRaises(ValueError):
            self.verify()

    def test_digest_and_identity_must_match(self):
        for field, value in [('sha256', '0' * 64), ('video_id', '2teKcIDANI4'),
                             ('declared_bytes', 14), ('verified_complete_audio', False)]:
            with self.subTest(field=field):
                row = dict(self.row, **{field: value})
                with self.assertRaises(ValueError):
                    qualify.verify_native(self.root, row, 'of4B3aw7MXo')

    def test_missing_trial_counts_as_failure(self):
        summary = qualify.summarize([dict(tool='agent-youtube', complete=False, reason='missing_result')])
        self.assertEqual(summary['agent-youtube'], dict(scheduled=1, attempted=1, complete=0, failed=1, skipped=0))

    def test_cooldown_skips_are_not_network_failures(self):
        summary = qualify.summarize([dict(tool='yt-dlp', complete=False, reason='skipped_server_cooldown')])
        self.assertEqual(summary['yt-dlp'], dict(scheduled=1, attempted=0, complete=0, failed=0, skipped=1))

    def test_reference_bot_denial_has_a_machine_readable_cause(self):
        self.assertEqual(qualify.reference_failure("Sign in to confirm you’re not a bot."), 'youtube_bot_check')
        self.assertEqual(qualify.reference_failure('HTTP Error 429'), 'youtube_rate_limited')

    def test_reference_exit_zero_cannot_hide_truncated_decoded_payload(self):
        args = SimpleNamespace(yt_dlp=pathlib.Path('yt-dlp'), yt_dlp_python=None,
                               node=None, ffmpeg=pathlib.Path('ffmpeg'), timeout=10)
        def successful_process(command, folder, label, timeout):
            if label == "reference":
                self.assertIn("--output-na-placeholder", command)
                self.assertEqual(command[command.index("--output-na-placeholder") + 1], "null")
            info = dict(id='of4B3aw7MXo', duration=1, format_id='140', filesize=13)
            return dict(exit_code=0, timeout=False, rate_limited=False), json.dumps(info)
        with patch.object(qualify, 'run', successful_process):
            self.assertTrue(qualify.reference(args, self.root, 'of4B3aw7MXo')['complete'])
            wav = self.root / 'audio.wav'
            wav.write_bytes(wav.read_bytes()[:-2])
            with self.assertRaises(ValueError):
                qualify.reference(args, self.root, 'of4B3aw7MXo')

    def test_provider_reference_is_explicit_and_reports_loading_separately(self):
        args = SimpleNamespace(yt_dlp=pathlib.Path('yt-dlp'), yt_dlp_python=None,
                               node=None, ffmpeg=pathlib.Path('ffmpeg'), timeout=10,
                               plugin_dir=pathlib.Path('/isolated/plugin'),
                               po_provider_url='http://127.0.0.1:14416', reference_client='mweb')
        def denied(command, folder, label, timeout):
            self.assertIn('--no-plugin-dirs', command)
            self.assertIn('/isolated/plugin', command)
            self.assertIn('youtube:player_client=mweb', command)
            self.assertIn('youtubepot-bgutilhttp:base_url=http://127.0.0.1:14416', command)
            self.assertEqual(command[command.index('--retries') + 1], '0')
            (folder / 'reference.stderr').write_text(
                '[debug] [youtube] [pot] PO Token Providers: bgutil:http-2.0.2 (external)\n'
                'Sign in to confirm you are not a bot.')
            return dict(exit_code=1, timeout=False, rate_limited=False), ''
        with patch.object(qualify, 'run', denied):
            result = qualify.reference(args, self.root, 'of4B3aw7MXo')
        self.assertEqual(result['reason'], 'youtube_bot_check')
        self.assertTrue(result['provider_loaded'])
        self.assertFalse(result['provider_token_requested'])

    def test_loaded_provider_is_not_evidence_of_a_token_request(self):
        evidence = qualify.provider_evidence(
            '[debug] PO Token Providers: bgutil:http-2.0.2 (external, unavailable)')
        self.assertFalse(evidence['provider_loaded'])
        evidence = qualify.provider_evidence(
            qualify.redact('[debug] PO Token Providers: bgutil:http-2.0.2 (external)\n'
            '[youtube] [pot:bgutil:http] Generating a gvs PO Token for mweb client via bgutil HTTP server\n'
            '[debug] Retrieved a gvs PO Token for mweb client'))
        self.assertTrue(evidence['provider_loaded'])
        self.assertTrue(evidence['provider_token_requested'])
        self.assertTrue(evidence['provider_token_received'])

    def test_schedule_keeps_comparisons_paired_and_reproducible(self):
        schedule = qualify.trial_schedule(['of4B3aw7MXo', '2teKcIDANI4'], 2, 42)
        self.assertEqual(schedule, qualify.trial_schedule(['of4B3aw7MXo', '2teKcIDANI4'], 2, 42))
        self.assertEqual(len(schedule), 8)
        for first, second in zip(schedule[::2], schedule[1::2]):
            self.assertEqual(first[1:], second[1:])
            self.assertEqual({first[0], second[0]}, {'agent-youtube', 'yt-dlp'})

    def test_native_comparison_can_pin_the_same_mweb_client(self):
        folder = self.root / 'native'
        folder.mkdir()
        args = SimpleNamespace(native_resume_after_bytes=None, native_client='mweb', timeout=10)
        def completed(command, output, label, timeout, env):
            self.assertEqual(env['AGENT_YOUTUBE_PRODUCTION_CLIENT'], 'mweb')
            media = folder / 'media'
            media.mkdir()
            for name in ['audio.m4a', 'audio.wav', 'audio.json']:
                (media / name).write_bytes((self.root / name).read_bytes())
            (media / 'result.json').write_text(json.dumps(dict(self.row, acquisition_seconds=0.4, decode_seconds=0.6)))
            return dict(exit_code=0, timeout=False, rate_limited=False), ''
        with patch.object(qualify, 'run', completed):
            result = qualify.native(args, pathlib.Path('probe'), folder, 'of4B3aw7MXo')
            self.assertTrue(result['complete'])
            self.assertEqual(result['native_stages'], dict(acquisition_seconds=0.4, decode_seconds=0.6))

    def test_signed_urls_are_redacted(self):
        value = qualify.redact('HTTP Error https://host/audio?pot=secret&expire=123 token=abc')
        self.assertNotIn('secret', value)
        self.assertNotIn('abc', value)

    def test_quoted_credentials_are_redacted(self):
        value = qualify.redact('{"po_token":"private-token", "cookie":"private-cookie"}')
        self.assertNotIn('private-token', value)
        self.assertNotIn('private-cookie', value)
        for value in ['Authorization: Bearer private-token',
                      'Cookie: first=private-token; second=private-cookie',
                      'credentials Bearer private-token']:
            with self.subTest(value=value):
                self.assertNotIn('private-token', qualify.redact(value))
                self.assertNotIn('private-cookie', qualify.redact(value))

    def test_provider_content_bindings_are_redacted_without_hiding_events(self):
        text = ('Generating POT for private-visitor-canary\n'
                'POT for private-cached-visitor still fresh, returning cached token\n'
                '{"content_binding":"private-binding-canary", "contentBinding":"private-camel-canary"}\n'
                'Generating POT for \n'
                '[pot:bgutil:http] Generating a gvs PO Token for mweb client via bgutil HTTP server\n')
        value = qualify.redact(text)
        for canary in ['private-visitor-canary', 'private-cached-visitor', 'private-binding-canary', 'private-camel-canary']:
            self.assertNotIn(canary, value)
        self.assertIn('Generating POT for [binding redacted]', value)
        self.assertIn('[pot:bgutil:http] Generating a gvs PO Token', value)
        self.assertIn('[binding redacted] still fresh, returning cached token', value)
        self.assertEqual(qualify.redact(value), value)


if __name__ == '__main__':
    unittest.main()
