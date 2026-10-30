#!/usr/bin/env python3
"""Exercise test-only SABR stalls against real audio; never a production option."""
import argparse
import json
import os
from pathlib import Path
import re
import time

import qualify as q
from series import save


def verify_stall(media, result, persistent):
    evidence = result['metrics']['stall']
    receipt = q.read_json(media / 'audio.json')
    prefix = evidence['prefix_bytes']
    cycles = (8, 12) if persistent else (4, 8)
    if (type(prefix) is not int or not 0 < prefix < receipt['identity']['length']
            or evidence['renewals'] != 1 or evidence['cycles'] not in cycles
            or evidence['persistent'] is not persistent
            or evidence['identity'] != receipt['identity']
            or q.digest(media / 'audio.m4a', prefix) != evidence['prefix_sha256']):
        raise ValueError('stall renewal or prefix evidence mismatch')
    if persistent and (result.get('error_code') != 'download_no_progress'
                       or result.get('retryable') is not True
                       or result['verified_complete_audio'] is not False
                       or receipt['progress']['bytes'] != prefix
                       or (media / 'audio.m4a').stat().st_size != prefix):
        raise ValueError('persistent stall was not safely exhausted')
    return evidence


def cooldown_active():
    cache = Path(os.environ.get('XDG_CACHE_HOME', str(Path.home() / '.cache')))
    path = cache / 'agent-youtube/state.json'
    return path.exists() and json.loads(path.read_text()).get('cooldown_until_ms', 0) > time.time() * 1000


def trial(args, folder, video, persistent):
    media = folder / 'media'
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(('AGENT_YOUTUBE_RESEARCH_', 'AGENT_YOUTUBE_PRODUCTION_', 'AGENT_YOUTUBE_CLIENT_'))}
    env.update(AGENT_YOUTUBE_CLIENT_PROBE_DIR=str(media), AGENT_YOUTUBE_CLIENT_VIDEO=video,
               AGENT_YOUTUBE_CLIENT_BINARY=str(args.binary), AGENT_YOUTUBE_PRODUCTION_CLIENT='auto',
               AGENT_YOUTUBE_RESEARCH_STALL='persistent' if persistent else 'renew',
               AGENT_YOUTUBE_RESEARCH_STALL_AFTER_BYTES=str(args.after_bytes))
    process, _ = q.run([str(args.probe), '--ignored', '--exact', q.PROBE, '--nocapture'],
                       folder, 'initial', args.timeout, env)
    row = dict(video_id=video, persistent=persistent, complete=False, initial=process)
    result = q.read_json(media / 'result.json')
    row['error_code'] = result.get('error_code')
    if process['exit_code'] or process['timeout'] or result.get('error_code') == 'youtube_rate_limited':
        return row
    evidence = verify_stall(media, result, persistent)
    row['injection'] = evidence
    row['initial_metrics'] = result['metrics']
    if persistent:
        # No manual lease/receipt changes. A new process reads the same failed job.
        env.pop('AGENT_YOUTUBE_RESEARCH_STALL')
        env.pop('AGENT_YOUTUBE_RESEARCH_STALL_AFTER_BYTES')
        time.sleep(max(1, result.get('retry_after_seconds') or 0))
        if cooldown_active():
            row['reason'] = 'skipped_resume_server_cooldown'
            return row
        resumed, _ = q.run([str(args.probe), '--ignored', '--exact', q.RESUME, '--nocapture'],
                           folder, 'resume', args.timeout, env)
        row['resume'] = resumed
        row['initial_error_code'] = row['error_code']
        result = q.read_json(media / 'result.resume.json')
        row['error_code'] = result.get('error_code')
        row['resume_metrics'] = result['metrics']
        if resumed['exit_code'] or resumed['timeout'] or not result.get('verified_complete_audio'):
            return row
        if result['metrics']['stall']['cycles'] != 0:
            raise ValueError('restart unexpectedly injected a stall')
        receipt = q.read_json(media / 'audio.json')
        if receipt['identity'] != evidence['identity']:
            raise ValueError('restart changed representation')
        result['declared_bytes'] = receipt['identity']['length']
    if q.digest(media / 'audio.m4a', evidence['prefix_bytes']) != evidence['prefix_sha256']:
        raise ValueError('final prefix mismatch')
    row.update(q.verify_native(media, result, video), complete=True, prefix_preserved=True)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--probe', type=Path, required=True)
    parser.add_argument('--videos', nargs='+', default=['of4B3aw7MXo', 'bedsdFbhqhw', 'a2hbJlPxj4M'])
    parser.add_argument('--persistent-video', default='bedsdFbhqhw')
    parser.add_argument('--after-bytes', type=int, default=65536)
    parser.add_argument('--timeout', type=int, default=1800)
    args = parser.parse_args()
    if args.after_bytes <= 0 or args.timeout <= 0 or any(not re.fullmatch(r'[A-Za-z0-9_-]{11}', v) for v in [*args.videos, args.persistent_video]):
        parser.error('positive limits and valid video IDs required')
    args.output, args.binary, args.probe = (p.resolve() for p in (args.output, args.binary, args.probe))
    if not args.output.is_relative_to((q.REPO / 'runs').resolve()):
        parser.error('output must be private runs data')
    os.umask(0o077)
    args.output.mkdir(mode=0o700)
    plan = [(v, False) for v in args.videos] + [(args.persistent_video, True)]
    report = dict(schema_version=1, finished=False, trials=[],
                  scheduled=[dict(video_id=v, persistent=p) for v, p in plan],
                  binary_sha256=q.digest(args.binary), probe_sha256=q.digest(args.probe),
                  after_bytes=args.after_bytes,
                  limitations=['synthetic no-progress cycles before dispatch, not natural server failures',
                               'one network; no claim of universal access; full native decode, no ASR'])
    save(args.output / 'report.json', report)
    for index, (video, persistent) in enumerate(plan):
        folder = args.output / f'{index:02}-{video}'
        folder.mkdir(mode=0o700)
        row = dict(video_id=video, persistent=persistent, complete=False)
        if cooldown_active():
            row['reason'] = 'skipped_server_cooldown'
        else:
            try:
                row = trial(args, folder, video, persistent)
            except Exception as error:
                row.update(reason='invalid_or_missing_evidence', exception_type=type(error).__name__)
        report['trials'].append(row)
        save(args.output / 'report.json', report)
        print(json.dumps({key: row.get(key) for key in ['video_id', 'persistent', 'complete', 'error_code', 'reason']}), flush=True)
        if row.get('error_code') == 'youtube_rate_limited':
            break
    report['finished'] = True
    report['complete'] = sum(row['complete'] for row in report['trials'])
    save(args.output / 'report.json', report)
    return 0 if report['complete'] == len(plan) else 1


if __name__ == '__main__':
    raise SystemExit(main())
