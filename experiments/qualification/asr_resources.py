#!/usr/bin/env python3
"""Observe only real GigaSTT batch children of an explicitly owned run driver."""
import argparse
import json
import os
from pathlib import Path
import time

from asr_restart import process, tree
import qualify as q
from series import save


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--driver', type=int, required=True)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--gigastt', type=Path, required=True)
    args = parser.parse_args()
    run, engine = args.run.resolve(), args.gigastt.resolve()
    if not run.is_relative_to(q.REPO / 'runs'):
        parser.error('owned ignored run required')
    try:
        argv = Path(f'/proc/{args.driver}/cmdline').read_bytes().split(b'\0')
    except FileNotFoundError:
        parser.error('driver is no longer live')
    if b'--output' not in argv or Path(os.fsdecode(argv[argv.index(b'--output') + 1])).resolve() != run:
        parser.error('driver does not own the supplied run')
    identity = process(args.driver)
    if identity is None:
        parser.error('driver is not live')
    os.umask(0o077)
    summary = dict(schema_version=1, recognizer_sha256=q.digest(engine), observer_sha256=q.digest(Path(__file__)),
                   interval_seconds=.5, phases={}, complete=False,
                   measurement='GigaSTT batch process RSS and total OS threads; CPU accounting from proc ticks')
    seen = {}
    last_save = 0
    with (run / 'engine-samples.jsonl').open('x') as output:
        while (parent := process(args.driver)) and parent['start'] == identity['start'] and parent['state'] != 'Z':
            for child in tree(args.driver)[1:]:
                try:
                    pid = child['pid']
                    if Path(f'/proc/{pid}/exe').resolve() != engine:
                        continue
                    command = Path(f'/proc/{pid}/cmdline').read_bytes().split(b'\0')
                    if b'transcribe-batch' not in command:
                        continue
                    fields = Path(f'/proc/{pid}/stat').read_bytes().rsplit(b')', 1)[1].split()
                    root_command = Path(f'/proc/{int(fields[1])}/cmdline').read_bytes().split(b'\0')
                    phase = 'initial' if root_command[1] == b'transcribe' else 'resume'
                    threads = next(int(line.split()[1]) for line in Path(f'/proc/{pid}/status').read_bytes().splitlines() if line.startswith(b'Threads:'))
                    ticks = int(fields[11]) + int(fields[12])
                    key = (pid, child['start'])
                    if key not in seen:
                        seen[key] = dict(first_ticks=ticks, last_ticks=ticks, phase=phase)
                    seen[key]['last_ticks'] = ticks
                    metric = summary['phases'].setdefault(phase, dict(peak_rss_bytes=0, peak_os_threads=0, samples=0))
                    metric['peak_rss_bytes'] = max(metric['peak_rss_bytes'], child['rss'])
                    metric['peak_os_threads'] = max(metric['peak_os_threads'], threads)
                    metric['samples'] += 1
                    output.write(json.dumps(dict(phase=phase, monotonic_seconds=time.monotonic(), rss_bytes=child['rss'], os_threads=threads, cpu_ticks=ticks)) + '\n')
                    output.flush()
                except FileNotFoundError:
                    continue
            now = time.monotonic()
            if now - last_save >= 10:
                save(run / 'engine-summary.json', summary)
                last_save = now
            time.sleep(.5)
    for phase, metric in summary['phases'].items():
        metric['observed_cpu_seconds'] = sum(row['last_ticks'] - row['first_ticks'] for row in seen.values() if row['phase'] == phase) / os.sysconf('SC_CLK_TCK')
    summary['complete'] = q.read_json(run / 'report.json').get('complete') is True
    save(run / 'engine-summary.json', summary)
    print(json.dumps(summary))
    return int(not summary['complete'])


if __name__ == '__main__':
    raise SystemExit(main())
