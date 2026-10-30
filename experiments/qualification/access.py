#!/usr/bin/env python3
"""Controlled access failures on real audio, separate from natural availability."""
import argparse
import json
import os
from pathlib import Path
import re

import qualify as q
from series import save
from stall import cooldown_active

KINDS = ('403', 'source-expiry', 'token-expiry', 'track', 'representation')


def verify_fault(media, result, kind):
    evidence = result['metrics']['access_fault']
    receipt = q.read_json(media / 'audio.json')
    prefix = evidence['prefix_bytes']
    if (evidence['kind'] != kind or evidence['injected'] is not True
            or type(prefix) is not int or not 0 < prefix < receipt['identity']['length']
            or evidence['identity'] != receipt['identity']
            or q.digest(media / 'audio.m4a', prefix) != evidence['prefix_sha256']):
        raise ValueError('missing injection or altered prefix/identity')
    if kind in ('track', 'representation'):
        expected = 'download_audio_track_changed' if kind == 'track' else 'download_representation_changed'
        if (result.get('error_code') != expected or result.get('retryable') is not False
                or result['verified_complete_audio'] is not False
                or evidence['changed_candidates'] < 1 or evidence['renewals'] != 0
                or receipt['progress']['bytes'] != prefix
                or (media / 'audio.m4a').stat().st_size != prefix):
            raise ValueError('incompatible source did not preserve a permanent refusal')
    elif kind == 'token-expiry':
        if evidence['token_refreshes'] < 1:
            raise ValueError('token expiry did not reach token issuance')
    elif evidence['renewals'] < 1:
        raise ValueError('source renewal was not exercised')
    return evidence


def trial(args, folder, kind):
    media = folder / 'media'
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(('AGENT_YOUTUBE_RESEARCH_', 'AGENT_YOUTUBE_PRODUCTION_', 'AGENT_YOUTUBE_CLIENT_'))}
    env.update(AGENT_YOUTUBE_CLIENT_PROBE_DIR=str(media), AGENT_YOUTUBE_CLIENT_VIDEO=args.video,
               AGENT_YOUTUBE_CLIENT_BINARY=str(args.binary), AGENT_YOUTUBE_PRODUCTION_CLIENT='auto',
               AGENT_YOUTUBE_RESEARCH_ACCESS_FAULT=kind, AGENT_YOUTUBE_RESEARCH_FAULT_AFTER_BYTES='65536')
    initial, _ = q.run([str(args.probe), '--ignored', '--exact', q.PROBE, '--nocapture'], folder, 'initial', args.timeout, env)
    row = dict(kind=kind, complete=False, initial=initial)
    result = q.read_json(media / 'result.json')
    row.update(error_code=result.get('error_code'), initial_metrics=result['metrics'])
    if initial['exit_code'] or initial['timeout'] or result.get('error_code') == 'youtube_rate_limited':
        return row
    evidence = verify_fault(media, result, kind)
    row['injection'] = evidence
    if kind in ('track', 'representation'):
        # Only synthetic candidate mutation is disabled. Durable media/receipt are untouched.
        env.pop('AGENT_YOUTUBE_RESEARCH_ACCESS_FAULT')
        env.pop('AGENT_YOUTUBE_RESEARCH_FAULT_AFTER_BYTES')
        if cooldown_active():
            row['reason'] = 'skipped_resume_server_cooldown'
            return row
        resumed, _ = q.run([str(args.probe), '--ignored', '--exact', q.RESUME, '--nocapture'], folder, 'resume', args.timeout, env)
        row.update(resume=resumed, initial_error_code=row['error_code'])
        result = q.read_json(media / 'result.resume.json')
        row.update(error_code=result.get('error_code'), resume_metrics=result['metrics'])
        if resumed['exit_code'] or resumed['timeout'] or not result.get('verified_complete_audio'):
            return row
        result['declared_bytes'] = evidence['identity']['length']
    if (q.digest(media / 'audio.m4a', evidence['prefix_bytes']) != evidence['prefix_sha256']
            or q.read_json(media / 'audio.json')['identity'] != evidence['identity']):
        raise ValueError('completion changed prefix or representation')
    row.update(q.verify_native(media, result, args.video), complete=True, prefix_preserved=True)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ['output', 'binary', 'probe']:
        parser.add_argument('--' + name, type=Path, required=True)
    parser.add_argument('--video', default='of4B3aw7MXo')
    parser.add_argument('--timeout', type=int, default=1800)
    args = parser.parse_args()
    args.output, args.binary, args.probe = (p.resolve() for p in (args.output, args.binary, args.probe))
    if not args.output.is_relative_to((q.REPO / 'runs').resolve()) or not re.fullmatch(r'[A-Za-z0-9_-]{11}', args.video) or args.timeout <= 0:
        parser.error('new private runs output, valid video and positive timeout required')
    os.umask(0o077)
    args.output.mkdir(mode=0o700)
    report = dict(schema_version=1, video_id=args.video, scheduled=list(KINDS), trials=[], finished=False,
                  binary_sha256=q.digest(args.binary), probe_sha256=q.digest(args.probe),
                  limitations=['synthetic client-side faults, real source acquisition and full decode',
                               'token expiry changes caller deadline; worker minter expiry tested offline separately',
                               'candidate identity mutated before compatibility check; no server response claim',
                               'one network; no ASR in these trials'])
    save(args.output / 'report.json', report)
    for kind in KINDS:
        folder = args.output / kind
        folder.mkdir(mode=0o700)
        row = dict(kind=kind, complete=False)
        if cooldown_active():
            row['reason'] = 'skipped_server_cooldown'
        else:
            try:
                row = trial(args, folder, kind)
            except Exception as error:
                row.update(reason='invalid_or_missing_evidence', exception_type=type(error).__name__)
        report['trials'].append(row)
        save(args.output / 'report.json', report)
        print(json.dumps({k: row.get(k) for k in ['kind', 'complete', 'error_code', 'reason']}), flush=True)
        if row.get('error_code') == 'youtube_rate_limited':
            break
    report.update(finished=True, complete=sum(r['complete'] for r in report['trials']))
    save(args.output / 'report.json', report)
    return 0 if report['complete'] == len(KINDS) else 1


if __name__ == '__main__':
    raise SystemExit(main())
