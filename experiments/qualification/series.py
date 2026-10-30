#!/usr/bin/env python3
"""Private finite local campaign. Frozen executables, no installs or hosted CI."""
import argparse
import datetime
import fcntl
import json
import os
import pathlib
import time
import wave
from types import SimpleNamespace
from zoneinfo import ZoneInfo
import corpus
import qualify as q


def save(path, value):
    temporary = path.with_suffix('.partial')
    with temporary.open('w') as stream:
        json.dump(value, stream, indent=2)
        stream.write('\n')
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def run(config_path):
    config = json.loads(config_path.read_text())
    q.REPO = pathlib.Path(config.get("repository", q.REPO)).resolve()
    root = pathlib.Path(config['output_root']).resolve()
    if not root.is_relative_to(q.REPO / 'runs'):
        raise ValueError('campaign output must be owned private runs data')
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    zone = ZoneInfo(config['timezone'])
    started = datetime.datetime.now(zone)
    date = started.date().isoformat()
    if date not in config['dates']:
        return dict(state='outside_finite_schedule', date=date)
    with (root / '.campaign.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return dict(state='already_running', date=date)
        folder = root / date
        if folder.exists():
            return dict(state='existing_series_preserved', date=date, report=str(folder / 'report.json'))
        folder.mkdir(mode=0o700)
        report = dict(schema_version=1, started_at=started.isoformat(), timezone=config['timezone'],
            corpus_sha256=config['corpus_sha256'], setup_complete=False, finished=False, trials=[],
            cohort=config['artifact_hashes'],
            limitations=['one network; no account or paid service', 'frozen configured VISIONOS reference vs native auto',
                         'reference request/retransmission counters are uninstrumented', 'no ASR accuracy claim'])
        target = folder / 'report.json'
        save(target, report)
        def record():
            report['counts'] = q.summarize(report['trials'])
            save(target, report)
            reports = [json.loads(p.read_text()) for p in sorted(root.glob('????-??-??/report.json'))]
            save(root / 'summary.json', corpus.aggregate(reports, len(config['dates'])))
        try:
            manifest_path = pathlib.Path(config['corpus'])
            manifest = corpus.load_manifest(manifest_path)
            if q.digest(manifest_path) != config['corpus_sha256']:
                raise ValueError('corpus changed')
            required = {'native_binary', 'probe', 'yt_dlp', 'node', 'ffmpeg', 'series_script', 'qualifier_script', 'corpus_module', 'manifest_file'}
            if set(config['artifact_hashes']) != required:
                raise ValueError('missing frozen artifact binding')
            for name, expected in config['artifact_hashes'].items():
                if q.digest(pathlib.Path(config[name])) != expected:
                    raise ValueError('frozen artifact changed')
            report['provenance'] = dict(config['artifact_hashes'], config_sha256=q.digest(config_path),
                                        harness_sha256=q.digest(pathlib.Path(__file__)), python=q.digest(pathlib.Path('/usr/bin/python3')))
            report['configuration'] = dict(reference_client='visionos', native_client='auto',
                resume_videos=config['resume_videos'], timeout=config['timeout'], reference_retries=0,
                reference_extractor_retries=0, request_spacing_seconds=5, provider=False)
            args = SimpleNamespace(yt_dlp=pathlib.Path(config['yt_dlp']), yt_dlp_python=pathlib.Path('/usr/bin/python3'),
                ffmpeg=pathlib.Path(config['ffmpeg']), node=pathlib.Path(config['node']), native_binary=pathlib.Path(config['native_binary']),
                native_client='auto', reference_client='visionos', plugin_dir=None, po_provider_url=None,
                native_resume_after_bytes=None, timeout=config['timeout'])
            report['setup_complete'] = True
            record()
            cutoff = time.monotonic() + config['max_series_seconds']
            cooldown = (root / 'pause.json').exists()
            for index, (tool, video, repetition) in enumerate(q.trial_schedule([r['id'] for r in manifest['videos']], 1, config['seed'])):
                row = dict(tool=tool, video_id=video, repetition=repetition, complete=False,
                    started_at=datetime.datetime.now(zone).isoformat())
                trial_folder = folder / f'{index:03d}-{tool}-{video}'
                trial_folder.mkdir(mode=0o700)
                cache = pathlib.Path(os.environ.get('XDG_CACHE_HOME', str(pathlib.Path.home() / '.cache'))) / 'agent-youtube/state.json'
                if cache.exists():
                    gate = json.loads(cache.read_text())
                    cooldown |= gate.get('cooldown_until_ms', 0) > time.time() * 1000
                if cooldown:
                    row['reason'] = 'skipped_server_cooldown'
                elif time.monotonic() + args.timeout > cutoff:
                    row['reason'] = 'skipped_series_time_budget'
                else:
                    args.native_resume_after_bytes = 1048576 if video in config['resume_videos'] else None
                    row['scenario'] = 'resume' if tool == 'agent-youtube' and args.native_resume_after_bytes else 'fresh'
                    try:
                        result = q.native(args, pathlib.Path(config['probe']), trial_folder, video) if tool == 'agent-youtube' else q.reference(args, trial_folder, video)
                        row.update(result)
                        row['total_wall_seconds'] = result.get('wall_seconds', 0) + (result.get('decode', {}).get('wall_seconds', 0) if tool == 'yt-dlp' else 0)
                    except (OSError, ValueError, KeyError, TypeError, wave.Error) as error:
                        row.update(reason='invalid_or_missing_evidence', exception_type=type(error).__name__)
                    # Reference headers are not exposed. Conservatively pause the
                    # campaign on observed cooldown/status503 rather than guessing.
                    for log in trial_folder.glob('*.stderr'):
                        import re
                        if re.search(r'HTTP(?: Error| error| status)?[ :]+(?:429|503)|Too Many Requests|youtube_rate_limited', log.read_text(), re.I):
                            row['rate_limited'] = True
                    if row.get('rate_limited'):
                        cooldown = True
                        save(root / 'pause.json', dict(reason='server_cooldown_requires_review', observed_at=row['started_at'],
                            video_id=video, tool=tool, next_action='inspect private trial and shared cooldown before resuming future dates'))
                row.setdefault('process_attempts', None)
                row.setdefault('metrics', dict(downloader_http_requests=None, verified_replayed_audio_bytes=None, media_wire_bytes=None))
                report['trials'].append(row)
                save(trial_folder / 'trial.json', row)
                record()
                print(json.dumps(dict(event='trial', tool=tool, video_id=video, complete=row['complete'], reason=row.get('reason'))), flush=True)
                if not cooldown and row.get('reason') != 'skipped_series_time_budget':
                    time.sleep(5)
            report['finished'] = True
        except (OSError, ValueError, KeyError, TypeError, wave.Error) as error:
            report['setup_error'] = type(error).__name__
        record()
        return dict(state='finished' if report['finished'] else 'setup_failed', report=str(target), counts=report.get('counts'))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True, type=pathlib.Path)
    arguments = parser.parse_args()
    print(json.dumps(run(arguments.config)), flush=True)
