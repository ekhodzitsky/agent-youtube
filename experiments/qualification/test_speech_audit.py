import unittest
import io
import json
from pathlib import Path
import tempfile

from speech_audit import gaps, candidates, record_regions, load_asr


class SpeechAuditTests(unittest.TestCase):
    def test_removed_speech_is_candidate_but_silence_is_not(self):
        words = [{"start": 0, "end": 2, "word": "before"},
                 {"start": 5, "end": 8, "word": "after"}]
        windows = gaps(words, 8)
        frames = [(i / 10, (i + 1) / 10, 0.9) for i in range(80)]
        result = list(candidates(windows, iter(frames)))
        self.assertEqual(len(result), 1)
        self.assertAlmostEqual(result[0]["start"], 2.2)
        self.assertAlmostEqual(result[0]["end"], 4.8)
        self.assertEqual(list(candidates(windows, ((s, e, 0.0) for s, e, _ in frames))), [])

    def test_overlap_zero_length_edges_and_bounded_retry_windows(self):
        words = [{"start": 0, "end": 10, "word": "a"},
                 {"start": 1, "end": 2, "word": "b"},
                 {"start": 12, "end": 12, "word": "c"}]
        windows = gaps(words, 100)
        self.assertAlmostEqual(windows[0][0], 10.2)
        self.assertTrue(all(e - s <= 20 + 1e-12 for s, e in windows))
        self.assertEqual(windows[-1][1], 100)

    def test_invalid_alignment_and_probabilities_are_rejected(self):
        for start, end in [(float("nan"), 1), (2, 1), (-1, 0), (0, 11)]:
            with self.assertRaises(ValueError):
                gaps([{"start": start, "end": end, "word": "x"}], 10)
        with self.assertRaises(ValueError):
            gaps([{"start": 2, "end": 3, "word": "a"},
                  {"start": 1, "end": 2, "word": "b"}], 10)
        with self.assertRaises(ValueError):
            list(candidates([(0, 2)], iter([(0, 2, float("nan"))])))

    def test_frames_crossing_windows_are_not_dropped(self):
        result = list(candidates([(0, 1), (1, 2)], iter([(0, 2, 0.9)])))
        self.assertEqual([r["speech_seconds"] for r in result], [1, 1])

    def test_regions_include_trailing_speech_and_reject_short_blips(self):
        out = io.StringIO()
        frames = [(0, 0.1, 0.9), (0.1, 0.2, 0), (0.2, 1, 0.9)]
        self.assertEqual(list(record_regions(iter(frames), out)), frames)
        self.assertEqual(json.loads(out.getvalue()), {"start": 0.2, "end": 1})

    def test_duration_mismatch_and_untimed_text_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "asr.json"
            path.write_text(json.dumps({"duration": 10, "words": [], "text": ""}))
            with self.assertRaises(ValueError):
                load_asr(path, 11)
            self.assertEqual(load_asr(path, 10)["words"], [])
            path.write_text(json.dumps({"duration": 10, "words": [], "text": "hello"}))
            with self.assertRaises(ValueError):
                load_asr(path, 10)


if __name__ == "__main__":
    unittest.main()
