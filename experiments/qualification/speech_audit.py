#!/usr/bin/env python3
"""Research only: independent Silero evidence, never a transcript replacement.

Mono PCM16/16k WAV is streamed. ASR JSON is explicitly limited to 16 MiB.
The production pipeline neither imports this tool nor depends on Python/ORT.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time
import wave

RATE = 16000
MODEL_SHA256 = "2623a2953f6ff3d2c1e61740c6cdb7168133479b267dfef114a4a3cc5bdd788f"


def digest(path):
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def gaps(words, duration):
    """Conservative 200ms word margin; split review windows into <=20s pieces."""
    if not math.isfinite(duration) or duration <= 0:
        raise ValueError("invalid duration")
    cursor, previous = 0.0, 0.0
    intervals = []
    for word in words:
        start, end = word["start"], word["end"]
        if (not all(math.isfinite(t) for t in (start, end))
                or not previous <= start <= end <= duration
                or not word["word"].strip()):
            raise ValueError("invalid word alignment")
        stop = max(0, start - 0.2)
        if stop - cursor >= 1:
            intervals.append((cursor, stop))
        cursor = max(cursor, min(duration, end + 0.2))
        previous = start
    if duration - cursor >= 1:
        intervals.append((cursor, duration))
    return [(start + offset, min(end, start + offset + 20))
            for start, end in intervals
            for offset in range(0, math.ceil(end - start), 20)]


def candidates(windows, frames):
    """Intersect independent speech probability frames with untimed windows.

    A candidate requires >=0.5s of model-positive audio, not merely loudness.
    Thresholds are research heuristics, not a calibrated omission classifier.
    """
    index, support = 0, 0.0
    for start, end, prob in frames:
        if not math.isfinite(prob) or not 0 <= prob <= 1:
            raise ValueError("invalid VAD probability")
        while index < len(windows) and windows[index][0] < end:
            if prob >= 0.5:
                support += max(0, min(end, windows[index][1]) - max(start, windows[index][0]))
            if windows[index][1] > end:
                break
            if support >= 0.5:
                yield dict(start=windows[index][0], end=windows[index][1], speech_seconds=support)
            index += 1
            support = 0.0
    while index < len(windows):
        if support >= 0.5:
            yield dict(start=windows[index][0], end=windows[index][1], speech_seconds=support)
        index += 1
        support = 0.0


def record_regions(frames, out):
    """Record >=250ms uninterrupted positive runs, without silence padding."""
    first, last = None, None
    for start, end, probability in frames:
        if probability >= 0.5:
            if first is None:
                first = start
            last = end
        elif first is not None:
            if last - first >= 0.25:
                out.write(json.dumps({"start": first, "end": last}) + "\n")
            first = None
        yield start, end, probability
    if first is not None and last - first >= 0.25:
        out.write(json.dumps({"start": first, "end": last}) + "\n")


def probabilities(audio, model, context, out):
    import numpy as np
    import onnxruntime as ort

    opts = ort.SessionOptions()
    opts.intra_op_num_threads = opts.inter_op_num_threads = 1
    session = ort.InferenceSession(str(model), opts, providers=["CPUExecutionProvider"])
    if {x.name for x in session.get_inputs()} != {"input", "state", "sr"}:
        raise ValueError("unexpected model inputs")
    state = np.zeros((2, 1, 128), dtype=np.float32)
    history = np.zeros((1, context), dtype=np.float32)
    with wave.open(str(audio), "rb") as wav:
        position = 0
        while data := wav.readframes(512):
            samples = np.frombuffer(data, dtype="<i2").astype(np.float32) / 32768
            count = len(samples)
            frame = np.pad(samples, (0, 512 - count)).reshape(1, 512)
            value = np.concatenate((history, frame), axis=1)
            prob, state = session.run(["output", "stateN"],
                                     {"input": value, "state": state,
                                      "sr": np.array(RATE, dtype=np.int64)})
            history = frame[:, -context:] if context else history
            p = float(prob.item())
            if not math.isfinite(p) or not 0 <= p <= 1 or not np.isfinite(state).all():
                raise ValueError("nonfinite or invalid model output")
            out.write(json.dumps({"start_frame": position, "end_frame": position + count,
                                  "speech_probability": p}) + "\n")
            yield position / RATE, (position + count) / RATE, p
            position += count
        if position != wav.getnframes():
            raise ValueError("truncated WAV payload")


def retry(audio, candidate, root, engine):
    """Keep the exact new result separate, with its original-timeline offset."""
    root.mkdir()
    with wave.open(str(audio), "rb") as source:
        start = max(0, int((candidate["start"] - 3) * RATE))
        end = min(source.getnframes(), math.ceil((candidate["end"] + 3) * RATE))
        source.setpos(start)
        with wave.open(str(root / "audio.wav"), "wb") as out:
            out.setparams(source.getparams())
            out.writeframes(source.readframes(end - start))
    env = {k: v for k, v in os.environ.items() if not k.startswith(("GIGASTT_", "ORT_"))}
    command = [str(engine), "--offline", "transcribe", str(root / "audio.wav"),
               "--model-variant", "e2e_rnnt", "--punctuation", "off", "--itn", "off",
               "--format", "json", "--output", str(root / "raw.json")]
    with (root / "stdout.log").open("x") as out, (root / "stderr.log").open("x") as err:
        result = subprocess.run(command, env=env, stdout=out, stderr=err, timeout=180)
    evidence = dict(start_frame=start, end_frame=end, exit_code=result.returncode,
                    candidate=candidate, accepted_as_correction=False,
                    audio_sha256=digest(root / "audio.wav"), engine_sha256=digest(engine))
    if result.returncode == 0:
        raw = root / "raw.json"
        data = load_asr(raw, (end - start) / RATE)
        evidence.update(raw_sha256=digest(raw), recognized_text=data["text"])
    (root / "evidence.json").write_text(json.dumps(evidence, ensure_ascii=False, indent=2))
    return evidence


def load_asr(path, duration):
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError("research ASR JSON limit is 16 MiB")
    data = json.loads(path.read_text())
    if not math.isfinite(data["duration"]) or abs(data["duration"] - duration) > 0.2:
        raise ValueError("ASR duration does not match audio")
    if data["text"].strip() and not data["words"]:
        raise ValueError("text has no word alignment; cannot classify uncovered speech")
    gaps(data["words"], duration)
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", type=Path)
    parser.add_argument("asr", type=Path)
    parser.add_argument("model", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--context", type=int, choices=[0, 64], default=64,
                        help="0 is a deliberately noncanonical comparison")
    parser.add_argument("--gigastt", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    before = {"audio_sha256": digest(args.audio), "asr_sha256": digest(args.asr),
              "model_sha256": digest(args.model)}
    if before["model_sha256"] != MODEL_SHA256:
        raise ValueError("expected pinned Silero v5.1.2 model")
    with wave.open(str(args.audio), "rb") as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, RATE):
            raise ValueError("expected mono PCM16 at 16 kHz")
        frames = wav.getnframes()
    duration = frames / RATE
    data = load_asr(args.asr, duration)
    windows = gaps(data["words"], duration)
    args.output.mkdir(parents=True, exist_ok=False)
    import numpy as np
    import onnxruntime as ort
    started = time.monotonic()
    engine_identity = None
    if args.gigastt:
        engine_identity = dict(sha256=digest(args.gigastt), version=subprocess.check_output(
            [str(args.gigastt), "--version"], timeout=10, text=True).strip())
    count, examples = 0, []
    with ((args.output / "frames.jsonl").open("x") as out,
          (args.output / "regions.jsonl").open("x") as regions,
          (args.output / "candidates.jsonl").open("x") as found):
        stream = record_regions(probabilities(args.audio, args.model, args.context, out), regions)
        for candidate in candidates(windows, stream):
            found.write(json.dumps(candidate) + "\n")
            count += 1
            if len(examples) < 5:
                examples.append(candidate)
    retries = []
    if args.gigastt:
        for index, candidate in enumerate(examples):
            retries.append(retry(args.audio, candidate, args.output / f"retry-{index}", args.gigastt))
    if before != {"audio_sha256": digest(args.audio), "asr_sha256": digest(args.asr),
                  "model_sha256": digest(args.model)}:
        raise ValueError("input changed during audit")
    report = dict(schema_version=1, status="research_only", speech_completeness="unverified",
                  frames=frames, sample_rate=RATE, context_samples=args.context,
                  candidate_count=count, retried_count=len(retries), retry_limit=5,
                  review_windows=len(windows), seconds=time.monotonic() - started,
                  word_margin_seconds=0.2, minimum_gap_seconds=1,
                  minimum_speech_seconds=0.5, probability_threshold=0.5,
                  region_method="uninterrupted_positive_runs_at_least_250ms",
                  identity_binding="explicit_input_pair_hashes_not_independent_source_authentication",
                  numpy=np.__version__, onnxruntime=ort.__version__, python=sys.version,
                  probe_sha256=digest(Path(__file__)), **before)
    report["recognizer"] = engine_identity
    (args.output / "report.json").write_text(json.dumps(report, indent=2))
    print(json.dumps(report))


if __name__ == "__main__":
    main()
