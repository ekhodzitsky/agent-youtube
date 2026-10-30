#!/usr/bin/env python3
"""Linux real-ASR hard-death/restart qualification; writes only owned local runs."""
import argparse
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import time

import qualify as q
from series import save
from stall import cooldown_active


def process(pid):
    try:
        fields = Path(f'/proc/{pid}/stat').read_bytes().rsplit(b')', 1)[1].split()
        return dict(pid=pid, state=fields[0].decode(), start=fields[19].decode(),
                    rss=max(0, int(fields[21])) * os.sysconf('SC_PAGE_SIZE'))
    except FileNotFoundError:
        return None


def tree(pid):
    result = []
    item = process(pid)
    if item is None:
        return result
    result.append(item)
    try:
        children = Path(f'/proc/{pid}/task/{pid}/children').read_text().split()
    except FileNotFoundError:
        return result
    for child in children:
        result.extend(tree(int(child)))
    return result


def send(item, sig):
    current = process(item['pid'])
    if current and current['start'] == item['start'] and current['state'] != 'Z':
        try:
            os.kill(item['pid'], sig)
        except ProcessLookupError:
            pass


def storage(root):
    """Allocated and apparent bytes deduplicated by inode, including hard links."""
    seen = set()
    apparent = allocated = 0
    for path in root.rglob('*'):
        try:
            st = path.stat(follow_symlinks=False)
            if not path.is_file() or path.is_symlink() or (st.st_dev, st.st_ino) in seen:
                continue
            seen.add((st.st_dev, st.st_ino))
            apparent += st.st_size
            allocated += st.st_blocks * 512
        except FileNotFoundError:
            pass
    return dict(apparent_bytes=apparent, allocated_bytes=allocated)


def outputs(work, chunk_seconds=30):
    result = {}
    for path in work.glob('batch.*/output/*.json'):
        try:
            row = json.loads(path.read_text())
            if isinstance(row.get('text'), str) and abs(row['duration'] - chunk_seconds) <= .2:
                result[path.name] = dict(path=str(path.relative_to(work)), sha256=q.digest(path))
        except (FileNotFoundError, json.JSONDecodeError, KeyError):
            pass
    return result


def transcript_json(path):
    # This is a qualification-reader memory bound, not a product duration limit.
    limit = 64 * 1024 * 1024
    with path.open('rb') as source:
        data = source.read(limit + 1)
    if len(data) > limit:
        raise ValueError('qualification transcript exceeds reader memory bound')
    return json.loads(data)


def publication(work, preserved):
    manifest = q.read_json(work / 'manifest.json')
    transcript = transcript_json(work / 'transcript.json')
    coverage = manifest['evidence']['coverage']
    assert q.digest(work / 'transcript.json') == coverage['transcript_sha256'] == manifest['evidence']['transcript_sha256']
    end = 0
    raw = {}
    for index, chunk in enumerate(manifest['chunks']):
        assert chunk['path'] == f'raw/{index:06}.json'
        assert chunk['input_start_frame'] == end
        end = chunk['input_end_frame']
        assert end > chunk['input_start_frame']
        name = Path(chunk['path']).name
        raw[name] = q.digest(work / chunk['path'])
        assert raw[name] == chunk['sha256']
    assert end == coverage['frames']
    assert len(raw) == coverage['chunks'] == math.ceil(end / coverage['chunk_frames'])
    assert abs(transcript['duration'] - end / coverage['sample_rate']) < 1e-6
    assert all(raw[name] == item['sha256'] for name, item in preserved.items())
    assert manifest['evidence']['download']['status'] == 'verified'
    assert not any(p.is_file() for name in ('media', 'chunks') for p in (work / name).rglob('*'))
    return dict(frames=end, decoded_seconds=transcript['duration'], raw_chunks=len(raw),
                raw_sha256=raw, transcript_sha256=q.digest(work / 'transcript.json'),
                configuration=manifest['evidence']['configuration'], producer=coverage['producer'])


def observe_submission(previous, current, preserved):
    assert not set(current).intersection(preserved)
    return sorted(set(previous).union(current))


class Progress:
    """Read bounded incremental JSONL; never persist messages or source text."""
    def __init__(self):
        self.offset = 0
        self.pending = b''
        self.stage = 'startup'
        self.completed_chunks = 0

    def read(self, path):
        with path.open('rb') as source:
            source.seek(self.offset)
            data = source.read(64 * 1024)
        self.offset += len(data)
        lines = (self.pending + data).split(b'\n')
        self.pending = lines.pop()
        if len(self.pending) > 64 * 1024:
            raise ValueError('oversized progress event')
        for line in lines:
            if not line:
                continue
            event = json.loads(line)
            stage = event.get('stage')
            if event.get('event') != 'progress':
                continue
            if stage in ['metadata', 'download']:
                self.stage = 'acquisition'
            elif stage in ['decode', 'decode_audio', 'prepare_chunks']:
                self.stage = 'decode'
            elif stage == 'recognize':
                self.stage = 'recognition'
                if type(event.get('completed_chunks')) is int:
                    self.completed_chunks = event['completed_chunks']
            elif stage in ['assemble', 'publish', 'complete']:
                self.stage = 'publication'
            elif stage == 'preflight':
                self.stage = 'preflight'


def observe_resources(phase, stage, members, interval, used=None):
    metric = phase['stages'].setdefault(stage, dict(sampled_seconds=0, peak_tree_rss_bytes=0,
              peak_parent_rss_bytes=0, peak_children_rss_bytes=0,
              peak_apparent_bytes=0, peak_allocated_bytes=0))
    metric['sampled_seconds'] += interval
    root = members[0]['rss'] if members else 0
    children = sum(p['rss'] for p in members[1:])
    metric['peak_tree_rss_bytes'] = max(metric['peak_tree_rss_bytes'], root + children)
    metric['peak_parent_rss_bytes'] = max(metric['peak_parent_rss_bytes'], root)
    metric['peak_children_rss_bytes'] = max(metric['peak_children_rss_bytes'], children)
    if used:
        for kind in ['apparent', 'allocated']:
            metric[f'peak_{kind}_bytes'] = max(metric[f'peak_{kind}_bytes'], used[f'{kind}_bytes'])


def initial_arguments(video, work, engine, default_settings):
    command = ['transcribe', f'https://www.youtube.com/watch?v={video}', '--work-dir', str(work),
               '--response', 'metadata']
    if not default_settings:
        command += ['--gigastt', str(engine), '--chunk-seconds', '30', '--encoder-threads', '4', '--retry-window-seconds', '180']
    return command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--gigastt', type=Path, required=True)
    parser.add_argument('--video', default='bedsdFbhqhw')
    parser.add_argument('--default-settings', action='store_true', help='omit ASR chunk/thread overrides; assert default300s and auto threads')
    parser.add_argument('--resume-job', type=Path, help='continue an owned failed-acquisition job instead of downloading afresh')
    parser.add_argument('--timeout', type=int, default=1800)
    args = parser.parse_args()
    chunk_seconds = 300 if args.default_settings else 30
    folder = args.output.resolve()
    if (not folder.is_relative_to((q.REPO / 'runs').resolve()) or args.timeout <= 0
            or not re.fullmatch(r'[A-Za-z0-9_-]{11}', args.video)):
        parser.error('new owned runs output, valid video and positive timeout required')
    os.umask(0o077)
    folder.mkdir(mode=0o700)
    binary, engine = args.binary.resolve(), args.gigastt.resolve()
    work = args.resume_job.resolve() if args.resume_job else folder / 'job'
    if args.resume_job:
        if not work.is_relative_to((q.REPO / 'runs').resolve()) or (work / 'transcript.json').exists():
            parser.error('resume job must be an unfinished owned runs job')
        config = q.read_json(work / 'state.json')['config']
        if (config['url'] != f'https://www.youtube.com/watch?v={args.video}'
                or config['gigastt_binary'] != q.digest(engine) or config['chunk_seconds'] != chunk_seconds):
            parser.error('resume job must match source, recognizer and chunk configuration')
    report = dict(schema_version=1, complete=False, binary_sha256=q.digest(binary),
                  recognizer_sha256=q.digest(engine), video_id=args.video, phases=[],
                  resumed_retained_acquisition=bool(args.resume_job), default_settings=args.default_settings,
                  expected_chunk_seconds=chunk_seconds,
                  sample_interval_seconds=.1, disk_interval_seconds=1,
                  measurement='sampled sum RSS (shared pages may be counted twice); inode-deduplicated owned-job storage')
    preserved = {}
    old_batches = set()
    child = None
    try:
        if cooldown_active():
            raise RuntimeError('shared server cooldown active')
        for label in ['initial', 'resume']:
            command = [str(binary), 'resume', '--work-dir', str(work), '--response', 'metadata']
            if label == 'initial' and not args.resume_job:
                command = [str(binary), *initial_arguments(args.video, work, engine, args.default_settings)]
            phase = dict(label=label, peak_rss_bytes=0, peak_apparent_bytes=0, peak_allocated_bytes=0, stages={})
            report['phases'].append(phase)
            start = last_disk = last_sample = last_checkpoint = time.monotonic()
            progress = Progress()
            with (folder / f'{label}.stdout').open('w') as out, (folder / f'{label}.stderr').open('w') as err:
                child = subprocess.Popen(command, stdout=out, stderr=err, start_new_session=True)
                while child.poll() is None:
                    progress.read(folder / f'{label}.stderr')
                    members = tree(child.pid)
                    phase['peak_rss_bytes'] = max(phase['peak_rss_bytes'], sum(p['rss'] for p in members))
                    now = time.monotonic()
                    observe_resources(phase, progress.stage, members, now - last_sample)
                    last_sample = now
                    if now - start > args.timeout:
                        raise TimeoutError('phase timeout; owned recovery data retained')
                    if now - last_disk >= 1:
                        used = storage(work)
                        observe_resources(phase, progress.stage, members, 0, used)
                        sample = dict(phase=label, stage=progress.stage, elapsed_seconds=now-start,
                            parent_rss_bytes=members[0]['rss'] if members else 0,
                            tree_rss_bytes=sum(p['rss'] for p in members), completed_chunks=progress.completed_chunks, **used)
                        with (folder / 'resource-samples.jsonl').open('a') as samples:
                            samples.write(json.dumps(sample) + '\n')
                        for kind in ['apparent', 'allocated']:
                            phase[f'peak_{kind}_bytes'] = max(phase[f'peak_{kind}_bytes'], used[f'{kind}_bytes'])
                        last_disk = now
                    if now - last_checkpoint >= 10:
                        report['current'] = dict(phase=label, stage=progress.stage, elapsed_seconds=now-start, completed_chunks=progress.completed_chunks)
                        save(folder / 'report.json', report)
                        last_checkpoint = now
                    if label == 'resume':
                        for batch in work.glob('batch.*'):
                            if batch.name not in old_batches:
                                names = sorted(p.with_suffix('.json').name for p in (batch / 'input').glob('*.wav'))
                                if names:
                                    report['resume_submitted_chunks'] = observe_submission(
                                        report.get('resume_submitted_chunks', []), names, preserved)
                    elif len(outputs(work, chunk_seconds)) >= 3:
                        # Freeze the parent, then its children before snapshotting exact bytes.
                        send(members[0], signal.SIGSTOP)
                        members = tree(child.pid)
                        for member in members[1:]:
                            send(member, signal.SIGSTOP)
                        preserved = outputs(work, chunk_seconds)
                        old_batches = {p.name for p in work.glob('batch.*')}
                        assert 3 <= len(preserved) < len(list((work / 'chunks').glob('*.wav')))
                        assert any(Path(f"/proc/{m['pid']}/exe").resolve() == engine for m in members[1:])
                        assert not (work / 'transcript.json').exists()
                        media = {str(p.relative_to(work)): q.digest(p) for p in (work / 'media').rglob('*') if p.is_file()}
                        assert media and list((work / 'chunks').glob('*.wav'))
                        report.update(preserved_before_kill=preserved, media_before_kill=media,
                                      killed_during_active_asr=True)
                        for member in reversed(members):
                            send(member, signal.SIGKILL)
                        child.wait(timeout=10)
                        for _ in range(100):
                            if not any((p := process(m['pid'])) and p['start'] == m['start'] and p['state'] != 'Z' for m in members):
                                break
                            time.sleep(.05)
                        else:
                            raise RuntimeError('owned worker teardown not proven')
                        assert all(q.digest(work / item['path']) == item['sha256'] for item in preserved.values())
                        assert all(q.digest(work / name) == sha for name, sha in media.items())
                        report['media_preserved_after_kill'] = True
                        break
                    time.sleep(.1)
                phase.update(exit_code=child.wait(), wall_seconds=time.monotonic() - start)
            save(folder / 'report.json', report)
            if label == 'initial':
                if not preserved and (folder / 'initial.stdout').stat().st_size:
                    report['initial_error_code'] = q.read_json(folder / 'initial.stdout').get('error', {}).get('code')
                assert preserved and phase['exit_code'] == -signal.SIGKILL
            else:
                assert phase['exit_code'] == 0 and q.read_json(folder / 'resume.stdout')['ok']
        report['verification_stage'] = 'publication'
        proof = publication(work, preserved)
        assert proof['configuration']['gigastt_binary'] == report['recognizer_sha256']
        assert proof['configuration']['chunk_seconds'] == chunk_seconds
        if args.default_settings:
            assert proof['configuration'].get('encoder_threads') is None
        report['verification_stage'] = 'submission_evidence'
        assert set(report['resume_submitted_chunks']) == set(proof['raw_sha256']) - set(preserved)
        report.update(complete=True, publication=proof, contiguous_full_coverage=True,
                      preserved_raw_bytes_verified=True, cleanup_complete=True)
    except Exception as error:
        report.update(failure_type=type(error).__name__)
        if child and child.poll() is None:
            for member in reversed(tree(child.pid)):
                send(member, signal.SIGKILL)
            child.wait(timeout=10)
    report.pop('current', None)
    save(folder / 'report.json', report)
    print(json.dumps({k: report.get(k) for k in ['complete', 'failure_type', 'phases']}), flush=True)
    return 0 if report['complete'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
