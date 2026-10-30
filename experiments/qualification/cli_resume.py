#!/usr/bin/env python3
"""Real CLI acquisition interruption, new-process resume, GigaSTT and publication."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import re
import time

import qualify as q
from series import save
from stall import cooldown_active


def validate_command(folder, label, process, stdout):
    value = json.loads(stdout)
    if type(value.get('ok')) is not bool or value.get('schema_version') not in (1, 2):
        raise ValueError('invalid CLI JSON envelope')
    for line in (folder / (label + '.stderr')).read_text().splitlines():
        event = json.loads(line)
        if event.get('schema_version') != 1 or not isinstance(event.get('event'), str):
            raise ValueError('invalid CLI JSONL event')
    if process['timeout'] or bool(process['exit_code'] == 0) != value['ok']:
        raise ValueError('CLI exit status mismatch')
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--video', default='bedsdFbhqhw')
    parser.add_argument('--timeout', type=int, default=1200)
    args = parser.parse_args()
    folder, binary = args.output.resolve(), args.binary.resolve()
    if not folder.is_relative_to((q.REPO / 'runs').resolve()) or not re.fullmatch(r'[A-Za-z0-9_-]{11}', args.video) or args.timeout <= 0:
        parser.error('new private runs output, valid video and positive timeout required')
    os.umask(0o077)
    folder.mkdir(mode=0o700)
    work = folder / 'job'
    report = dict(schema_version=1, complete=False, commands=[], binary_sha256=q.digest(binary),
                  video_id=args.video, interruption='explicit cancellation after durable acquisition prefix',
                  real_gigastt=True)
    def record():
        save(folder / 'report.json', report)
    def launch(label, arguments):
        return q.run([str(binary), *map(str, arguments)], folder, label, args.timeout)
    def finish(label, outcome):
        process, stdout = outcome
        value = validate_command(folder, label, process, stdout)
        report['commands'].append(dict(label=label, **process, ok=value['ok'],
                                       error_code=value.get('error', {}).get('code'), protocol_valid=True))
        record()
        return value
    def command(label, arguments):
        return finish(label, launch(label, arguments))
    record()
    try:
        if cooldown_active():
            raise RuntimeError('shared server cooldown active')
        with ThreadPoolExecutor(max_workers=1) as executor:
            first = executor.submit(launch, 'initial', ['transcribe', f'https://www.youtube.com/watch?v={args.video}',
                                    '--work-dir', work, '--chunk-seconds', '30', '--encoder-threads', '2',
                                    '--response', 'metadata'])
            observed = False
            while not first.done():
                for path in (work / 'media').glob('download.*.json'):
                    try:
                        receipt = q.read_json(path)
                        if 65536 <= receipt['progress']['bytes'] < receipt['identity']['length']:
                            cancel = command('cancel', ['cancel', '--work-dir', work])
                            if not cancel['ok']:
                                raise ValueError('cancellation failed')
                            observed = True
                            break
                    except FileNotFoundError:
                        pass
                if observed:
                    break
                time.sleep(.02)
            initial = finish('initial', first.result())
            if not observed or initial.get('error', {}).get('code') != 'interrupted':
                raise ValueError('acquisition interruption not exercised')
            receipts = list((work / 'media').glob('download.*.json'))
            if len(receipts) != 1:
                raise ValueError('ambiguous retained acquisition')
            receipt = q.read_json(receipts[0])
            prefix = receipt['progress']['bytes']
            sha = receipt['progress']['digest']
            if not 0 < prefix < receipt['identity']['length'] or q.digest(receipts[0].with_suffix('.m4a'), prefix) != sha:
                raise ValueError('invalid retained prefix')
            if (work / 'transcript.json').exists() or list((work / 'raw').glob('*.json')):
                raise ValueError('interruption occurred after recognition')
            report.update(prefix_bytes=prefix, prefix_sha256=sha, media_preserved_after_failure=True)
            record()
            resumed = executor.submit(launch, 'resume', ['resume', '--work-dir', work, '--response', 'metadata'])
            prefix_checked = False
            while not resumed.done():
                source = work / 'media/source.audio'
                if not prefix_checked and source.exists():
                    if source.stat().st_size != receipt['identity']['length'] or q.digest(source, prefix) != sha:
                        raise ValueError('resumed source changed retained prefix')
                    prefix_checked = True
                time.sleep(.02)
            result = finish('resume', resumed.result())
            if not result['ok'] or not prefix_checked:
                raise ValueError('resumed acquisition was not verified before cleanup')
        status = command('status', ['status', '--work-dir', work])
        state = status['result']
        if not (state['state'] == 'completed' and state['transcript_validated'] and state['cleanup_complete']):
            raise ValueError('publication or cleanup incomplete')
        if any(p.is_file() for name in ('media', 'chunks') for p in (work / name).rglob('*')):
            raise ValueError('owned media remains after completed cleanup')
        manifest = json.loads((work / 'manifest.json').read_text())
        transcript = json.loads((work / 'transcript.json').read_text())
        for chunk in manifest['chunks']:
            if q.digest(work / chunk['path']) != chunk['sha256']:
                raise ValueError('raw ASR hash mismatch')
        transcript_hash = q.digest(work / 'transcript.json')
        if transcript_hash != manifest['evidence']['transcript_sha256']:
            raise ValueError('transcript evidence mismatch')
        before = {p.name: q.digest(p) for p in (work / 'raw').glob('*.json')}
        reuse = command('completed-resume', ['resume', '--work-dir', work, '--response', 'metadata'])
        if not reuse['ok'] or q.digest(work / 'transcript.json') != transcript_hash or any(q.digest(work / 'raw' / name) != sha for name, sha in before.items()):
            raise ValueError('completed reuse changed published transcript or raw ASR')
        report.update(complete=True, prefix_preserved=True, cleanup_complete=True, transcript_validated=True,
                      transcript_sha256=transcript_hash, raw_files=len(before), decoded_seconds=transcript['duration'],
                      download=manifest['evidence']['download'])
    except Exception as error:
        report.update(failure_type=type(error).__name__)
    record()
    print(json.dumps(report), flush=True)
    return 0 if report['complete'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
