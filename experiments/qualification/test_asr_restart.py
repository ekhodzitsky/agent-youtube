import json
import os
from pathlib import Path
import tempfile
import unittest

from asr_restart import publication, storage, observe_submission, outputs, initial_arguments, Progress, observe_resources
import qualify as q


class AsrRestartEvidenceTests(unittest.TestCase):
    def fixture(self, root):
        (root / 'raw').mkdir()
        chunks = []
        for index in range(2):
            path = root / f'raw/{index:06}.json'
            path.write_text(json.dumps({'duration': 30, 'text': 'fixture'}))
            chunks.append(dict(path=str(path.relative_to(root)), sha256=q.digest(path),
                               input_start_frame=index * 480000, input_end_frame=(index + 1) * 480000))
        (root / 'transcript.json').write_text('{"duration":60}')
        sha = q.digest(root / 'transcript.json')
        manifest = dict(evidence=dict(coverage=dict(frames=960000, sample_rate=16000, chunk_frames=480000,
                          chunks=2, transcript_sha256=sha, producer={}), transcript_sha256=sha,
                          download=dict(status='verified'), configuration={}), chunks=chunks)
        (root / 'manifest.json').write_text(json.dumps(manifest))
        return manifest, {'000000.json': {'sha256': chunks[0]['sha256']}}

    def test_staging_cleanup_cannot_erase_observed_submission_evidence(self):
        observed = observe_submission([], ['000003.json', '000004.json'], {'000000.json'})
        self.assertEqual(observe_submission(observed, ['000004.json'], {'000000.json'}),
                         ['000003.json', '000004.json'])
        with self.assertRaises(AssertionError):
            observe_submission(observed, ['000000.json'], {'000000.json'})

    def test_default_scenario_does_not_override_actual_cli_defaults(self):
        default = initial_arguments('bedsdFbhqhw', Path('job'), Path('engine'), True)
        self.assertNotIn('--chunk-seconds', default)
        self.assertNotIn('--encoder-threads', default)
        self.assertNotIn('--gigastt', default)
        self.assertNotIn('--retry-window-seconds', default)
        legacy = initial_arguments('bedsdFbhqhw', Path('job'), Path('engine'), False)
        self.assertEqual(legacy[legacy.index('--chunk-seconds') + 1], '30')
        self.assertEqual(legacy[legacy.index('--encoder-threads') + 1], '4')

    def test_partial_progress_is_read_incrementally_without_changing_stage(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'stderr'
            path.write_bytes(b'{"event":"progress","stage":"decode"')
            progress = Progress()
            progress.read(path)
            self.assertEqual(progress.stage, 'startup')
            with path.open('ab') as output:
                output.write(b'}\n{"event":"progress","stage":"recognize","completed_chunks":2}\n')
            progress.read(path)
            self.assertEqual((progress.stage, progress.completed_chunks), ('recognition', 2))
            progress.read(path)
            self.assertEqual(progress.completed_chunks, 2)

    def test_resource_stages_do_not_mix_process_peaks_or_intervals(self):
        phase = {'stages': {}}
        observe_resources(phase, 'decode', [{'rss':100}], .1, {'apparent_bytes':500,'allocated_bytes':512})
        observe_resources(phase, 'recognition', [{'rss':200},{'rss':300}], .2)
        self.assertEqual(phase['stages']['decode']['peak_tree_rss_bytes'], 100)
        self.assertEqual(phase['stages']['recognition']['peak_tree_rss_bytes'], 500)
        self.assertEqual(phase['stages']['recognition']['peak_children_rss_bytes'], 300)
        self.assertEqual(phase['stages']['decode']['sampled_seconds'], .1)

    def test_outputs_follow_the_selected_chunk_duration(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            out = root / 'batch.1.2.3/output'
            out.mkdir(parents=True)
            (out / '000000.json').write_text('{"duration":300,"text":"fixture"}')
            (out / '000001.json').write_text('{"duration":30,"text":"wrong span"}')
            self.assertEqual(set(outputs(root, 300)), {'000000.json'})
            self.assertEqual(set(outputs(root)), {'000001.json'})

    def test_complete_contiguous_publication(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            _, preserved = self.fixture(root)
            self.assertEqual(publication(root, preserved)['raw_chunks'], 2)

    def test_transcript_larger_than_small_probe_result_is_valid(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            manifest, preserved = self.fixture(root)
            (root / 'transcript.json').write_text('{"duration":60}' + ' ' * 70000)
            sha = q.digest(root / 'transcript.json')
            manifest['evidence']['transcript_sha256'] = sha
            manifest['evidence']['coverage']['transcript_sha256'] = sha
            (root / 'manifest.json').write_text(json.dumps(manifest))
            self.assertEqual(publication(root, preserved)['raw_chunks'], 2)

    def test_gap_and_overlap_are_rejected(self):
        for offset in [-1, 1]:
            with self.subTest(offset=offset), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                manifest, preserved = self.fixture(root)
                manifest['chunks'][1]['input_start_frame'] += offset
                (root / 'manifest.json').write_text(json.dumps(manifest))
                with self.assertRaises(AssertionError):
                    publication(root, preserved)

    def test_raw_mutation_and_media_before_cleanup_are_rejected(self):
        for mutation in ['raw', 'media']:
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as folder:
                root = Path(folder)
                _, preserved = self.fixture(root)
                if mutation == 'raw':
                    (root / 'raw/000000.json').write_text('{}')
                else:
                    (root / 'media').mkdir()
                    (root / 'media/source.audio').write_bytes(b'owned fixture')
                with self.assertRaises(AssertionError):
                    publication(root, preserved)

    def test_replaced_raw_is_rejected_even_with_a_new_matching_manifest(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            manifest, preserved = self.fixture(root)
            (root / 'raw/000000.json').write_text('{"duration":30,"text":"replacement"}')
            manifest['chunks'][0]['sha256'] = q.digest(root / 'raw/000000.json')
            (root / 'manifest.json').write_text(json.dumps(manifest))
            with self.assertRaises(AssertionError):
                publication(root, preserved)

    def test_allocated_storage_deduplicates_staged_hard_links(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            original = root / 'chunk.wav'
            original.write_bytes(b'x' * 8192)
            os.link(original, root / 'staged.wav')
            self.assertEqual(storage(root), dict(apparent_bytes=8192,
                             allocated_bytes=original.stat().st_blocks * 512))


if __name__ == '__main__':
    unittest.main()
