#!/usr/bin/env python3
"""Local research only. Python/yt-dlp/FFmpeg are never production dependencies."""
import argparse
import hashlib
import json
import math
import os
import pathlib
import random
import re
import signal
import subprocess
import sys
import time
import wave
from urllib.parse import urlsplit

REPO = pathlib.Path(__file__).resolve().parents[2]
PROBE = 'download::tests::research_production_client'
RESUME = 'download::tests::research_production_resume'
REQUEST_CATEGORIES = ('watch', 'player', 'script', 'attestation', 'media', 'other')
METRIC_COVERAGE = 'downloader request attempts include injected DNS failures and exclude interpreter-worker HTTP; replay counts verified retained audio only, not failed partial bodies or TCP retransmissions'


def sum_request_categories(metrics):
    if not metrics:
        return None
    result = dict.fromkeys(REQUEST_CATEGORIES, 0)
    for metric in metrics:
        if not isinstance(metric, dict):
            return None
        counts = metric.get('requests_by_endpoint')
        if not isinstance(counts, dict) or set(counts) != set(REQUEST_CATEGORIES) or any(type(counts[key]) is not int or counts[key] < 0 for key in REQUEST_CATEGORIES):
            return None
        if type(metric.get('downloader_http_requests')) is not int or sum(counts[key] for key in REQUEST_CATEGORIES) != metric['downloader_http_requests']:
            return None
        for key in REQUEST_CATEGORIES:
            result[key] += counts[key]
    return result


def digest(path, limit=None):
    hasher = hashlib.sha256()
    with path.open('rb') as source:
        while limit is None or limit > 0:
            block = source.read(65536 if limit is None else min(65536, limit))
            if not block:
                if limit:
                    raise ValueError('short committed prefix')
                break
            hasher.update(block)
            if limit is not None:
                limit -= len(block)
    return hasher.hexdigest()


def redact(text):
    text = re.sub(r'(?im)(\bPOT for[ \t]+)(?!\[binding redacted\])[^ \t\r\n]+',
                  r'\1[binding redacted]', text)
    text = re.sub(r'https?://[^\s"<>]+', '[URL redacted]', text)
    text = re.sub(r'(?im)\b(?:authorization|cookie|set-cookie)\s*:\s*[^\r\n]+',
                  '[credential header redacted]', text)
    text = re.sub(r'(?i)\b(?:Bearer|Basic)\s+[A-Za-z0-9._~+/-]+=*',
                  '[authorization redacted]', text)
    return re.sub(
        r'''(?i)((?<![\w\[])["']?(?:po_token|poToken|integrityToken|token|cookie|authorization|visitor_data|visitorData|content_binding|contentBinding|pot)["']?\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,}]+)''',
        r'\1"[redacted]"', text)


def read_json(path):
    if path.stat().st_size > 65536:
        raise ValueError('oversized probe result')
    return json.loads(path.read_text())


def pcm(path):
    with wave.open(str(path), 'rb') as wav:
        if (wav.getnchannels(), wav.getsampwidth(), wav.getframerate()) != (1, 2, 16000):
            raise ValueError('unexpected PCM representation')
        frames = wav.getnframes()
        total = 0
        while block := wav.readframes(65536):
            total += len(block)
        if total != frames * 2 or frames == 0:
            raise ValueError('truncated or empty PCM payload')
        return frames / 16000


def verify_native(folder, row, video):
    receipt = read_json(folder / 'audio.json')
    source = folder / 'audio.m4a'
    size = source.stat().st_size
    sha256 = digest(source)
    seconds = pcm(folder / 'audio.wav')
    expected = row['declared_seconds']
    if (row['verified_complete_audio'] is not True or row['video_id'] != video
            or receipt['identity']['video_id'] != video
            or not math.isfinite(expected) or expected <= 0
            or abs(seconds - expected) > 2 or seconds != row['decoded_seconds']
            or size != row['bytes'] or size != row['declared_bytes']
            or size != receipt['identity']['length'] or size <= 0
            or sha256 != row['sha256']):
        raise ValueError('native completeness evidence mismatch')
    return dict(bytes=size, sha256=sha256, decoded_seconds=seconds,
                declared_seconds=expected, representation=receipt['identity'])


def summarize(rows):
    result = {}
    for row in rows:
        counts = result.setdefault(row['tool'], dict(scheduled=0, attempted=0, complete=0, failed=0, skipped=0))
        counts['scheduled'] += 1
        if (row.get('reason') or '').startswith('skipped_'):
            counts['skipped'] += 1
        else:
            counts['attempted'] += 1
            counts['complete' if row.get('complete') is True else 'failed'] += 1
    return result


def run(command, folder, label, timeout, env=None):
    started = time.monotonic()
    process = subprocess.Popen(command, cwd=REPO, env=env, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, start_new_session=True)
    timed_out = False
    try:
        out, err = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        timed_out = True
        os.killpg(process.pid, signal.SIGTERM)
        try:
            out, err = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            out, err = process.communicate()
    stdout, stderr = (redact(value.decode('utf-8', errors='replace')) for value in (out, err))
    (folder / (label + '.stdout')).write_text(stdout[:1048576])
    (folder / (label + '.stderr')).write_text(stderr[:1048576])
    limited = bool(re.search(r'HTTP(?: Error| error| status)?[ :]+429|Too Many Requests|youtube_rate_limited', stderr, re.I))
    return dict(exit_code=process.returncode, timeout=timed_out,
                wall_seconds=time.monotonic() - started, rate_limited=limited), stdout


def native(args, executable, folder, video):
    # The Rust hook creates its media directory exclusively. Runner logs live next to it.
    media = folder / 'media'
    env = dict(os.environ, AGENT_YOUTUBE_CLIENT_PROBE_DIR=str(media), AGENT_YOUTUBE_CLIENT_VIDEO=video,
               AGENT_YOUTUBE_CLIENT_BINARY=str(getattr(args, 'native_binary', REPO / 'target/release/agent-youtube')),
               AGENT_YOUTUBE_PRODUCTION_CLIENT=getattr(args, 'native_client', 'auto'))
    for key in ['AGENT_YOUTUBE_PRODUCTION_STOP_AFTER', 'AGENT_YOUTUBE_CLIENT_STOP_AFTER']:
        env.pop(key, None)
    if args.native_resume_after_bytes:
        env['AGENT_YOUTUBE_PRODUCTION_STOP_AFTER'] = str(args.native_resume_after_bytes)
    trial, _ = run([str(executable), '--ignored', '--exact', PROBE, '--nocapture'],
                   folder, 'native', args.timeout, env)
    # A killed probe may leave no result, or an old result cannot prove success
    # for this invocation. Preserve process timing/cause before reading evidence.
    if trial['timeout']:
        return dict(trial, complete=False, reason='native_trial_timeout', process_attempts=1)
    row = read_json(media / 'result.json')
    trial.update(metrics=row.get('metrics', {}), category_evidence=row.get('category_evidence'), process_attempts=1)
    trial['rate_limited'] |= row.get('error_code') == 'youtube_rate_limited'
    trial['error_code'] = row.get('error_code')
    for key in ['http_status', 'retry_after_seconds']:
        trial[key] = row.get(key) if type(row.get(key)) is int else None
    if trial['exit_code'] or trial['timeout'] or trial['rate_limited']:
        return dict(trial, complete=False, reason=row.get('error_code', 'probe_failed'))
    if args.native_resume_after_bytes:
        receipt = read_json(media / 'audio.json')
        progress = receipt['progress']
        length = receipt['identity']['length']
        if (row.get('cancelled') is not True or row['verified_complete_audio'] is not False
                or not 0 < progress['bytes'] < length
                or digest(media / 'audio.m4a', progress['bytes']) != progress['digest']):
            return dict(trial, complete=False, reason='interruption_not_exercised')
        env.pop('AGENT_YOUTUBE_PRODUCTION_STOP_AFTER', None)
        resumed, _ = run([str(executable), '--ignored', '--exact', RESUME, '--nocapture'],
                         folder, 'resume', args.timeout, env)
        final = read_json(media / 'result.resume.json')
        trial['initial_error_code'] = trial.get('error_code')
        trial['error_code'] = final.get('error_code')
        for key in ['http_status', 'retry_after_seconds']:
            trial[key] = final.get(key) if type(final.get(key)) is int else None
        trial['wall_seconds'] += resumed['wall_seconds']
        first_metrics, last_metrics = row.get('metrics', {}), final.get('metrics', {})
        final['category_evidence'] = row.get('category_evidence')
        final['metrics'] = {key: first_metrics[key] + last_metrics[key] for key in ['downloader_http_requests', 'verified_replayed_audio_bytes', 'injected_dns_failures', 'cdn_fallback_attempts', 'cdn_fallback_selections'] if type(first_metrics.get(key)) is int and type(last_metrics.get(key)) is int}
        final['metrics'].update(requests_by_endpoint=sum_request_categories([first_metrics, last_metrics]), media_wire_bytes=None, coverage=METRIC_COVERAGE)
        trial.update(metrics=final['metrics'], process_attempts=2)
        trial['resume'] = resumed
        trial['rate_limited'] |= resumed['rate_limited'] or final.get('error_code') == 'youtube_rate_limited'
        if resumed['exit_code'] or resumed['timeout'] or final.get('verified_complete_audio') is not True:
            return dict(trial, complete=False, reason=final.get('error_code', 'resume_failed'))
        if digest(media / 'audio.m4a', progress['bytes']) != progress['digest']:
            raise ValueError('resume changed committed prefix')
        final['declared_bytes'] = length
        final['declared_seconds'] = row['declared_seconds']
        row = final
        trial['resumed_from_bytes'] = progress['bytes']
    if row.get('verified_complete_audio') is not True:
        return dict(trial, complete=False, reason=row.get('error_code', 'probe_failed'))
    proof = verify_native(media, row, video)
    trial['native_stages'] = {key: row[key] for key in ['acquisition_seconds', 'decode_seconds'] if key in row}
    return dict(trial, complete=True, metrics=row.get('metrics', {}), category_evidence=row.get('category_evidence'), process_attempts=2 if args.native_resume_after_bytes else 1, **proof)


def reference_failure(stderr, timed_out=False):
    if timed_out:
        return 'reference_timeout'
    if re.search(r'429|Too Many Requests', stderr, re.I):
        return 'youtube_rate_limited'
    if 'not a bot' in stderr:
        return 'youtube_bot_check'
    if re.search(r'HTTP Error 403', stderr, re.I):
        return 'download_http_forbidden'
    if re.search(r'video unavailable|private video|has been removed', stderr, re.I):
        return 'youtube_source_unavailable'
    return 'reference_failed'


def provider_evidence(stderr):
    loaded = bool(re.search(r'bgutil:http-[^\s,]+ \(external\)', stderr))
    requested = bool(re.search(r'Generating a (?:gvs|player|subs) PO Token for \w+ client via bgutil HTTP server', stderr))
    return dict(
        provider_loaded=loaded, provider_token_requested=requested,
        provider_token_received=loaded and requested and bool(re.search(r'Retrieved a (?:gvs|player|subs) PO Token for \w+ client', stderr)))


def trial_schedule(videos, repetitions, seed):
    rng = random.Random(seed)
    pairs = [(video, rep) for rep in range(repetitions) for video in videos]
    rng.shuffle(pairs)
    schedule = []
    for video, rep in pairs:
        tools = ['agent-youtube', 'yt-dlp']
        rng.shuffle(tools)
        schedule.extend((tool, video, rep) for tool in tools)
    return schedule


def reference(args, folder, video):
    command = ([str(args.yt_dlp_python)] if args.yt_dlp_python else []) + [str(args.yt_dlp), '--ignore-config', '--no-cache-dir', '--no-playlist',
               '--no-progress', '--no-warnings', '--no-plugin-dirs', '--output-na-placeholder', 'null', '--socket-timeout', '30',
               '--retries', '0', '--extractor-retries', '0', '--sleep-requests', '5',
               '--fixup', 'never', '-f', 'bestaudio[ext=m4a]', '-o', str(folder / 'audio.%(ext)s'),
               '--print', 'after_move:{"id":%(id)j,"duration":%(duration)j,"format_id":%(format_id)j,"filesize":%(filesize)j,"language":%(language)j,"format_note":%(format_note)j,"upload_date":%(upload_date)j,"audio_languages":%(formats.:.language)j}']
    if args.node:
        command += ['--js-runtimes', 'node:' + str(args.node)]
    if getattr(args, 'reference_client', 'default') != 'default':
        command += ['--extractor-args', 'youtube:player_client=' + args.reference_client]
    if getattr(args, 'plugin_dir', None):
        command += ['--plugin-dirs', str(args.plugin_dir), '--verbose']
    if getattr(args, 'po_provider_url', None):
        command += ['--extractor-args', 'youtubepot-bgutilhttp:base_url=' + args.po_provider_url]
    command += ['https://www.youtube.com/watch?v=' + video]
    trial, stdout = run(command, folder, 'reference', args.timeout)
    trial['process_attempts'] = 1
    stderr = (folder / 'reference.stderr').read_text() if (folder / 'reference.stderr').exists() else ''
    trial.update(provider_evidence(stderr))
    if getattr(args, 'plugin_dir', None) and not trial['provider_loaded']:
        return dict(trial, complete=False, reason='reference_provider_unavailable')
    if trial['exit_code'] or trial['timeout']:
        return dict(trial, complete=False, reason=reference_failure(stderr, trial['timeout']))
    info = json.loads(stdout.strip().splitlines()[-1])
    source = folder / 'audio.m4a'
    if info['id'] != video or source.stat().st_size != info['filesize']:
        raise ValueError('reference identity or byte count mismatch')
    decoded, _ = run([str(args.ffmpeg), '-v', 'error', '-xerror', '-nostdin', '-i', str(source),
                      '-vn', '-ac', '1', '-ar', '16000', '-c:a', 'pcm_s16le', str(folder / 'audio.wav')],
                     folder, 'decode', args.timeout)
    trial['decode'] = decoded
    if decoded['exit_code'] or decoded['timeout']:
        return dict(trial, complete=False, reason='decode_failed')
    seconds = pcm(folder / 'audio.wav')
    expected = info['duration']
    if not isinstance(expected, (int, float)) or not math.isfinite(expected) or expected <= 0 or abs(seconds - expected) > 2:
        raise ValueError('reference decoded duration mismatch')
    return dict(trial, complete=True, bytes=source.stat().st_size, sha256=digest(source),
                declared_seconds=expected, decoded_seconds=seconds, format_id=info['format_id'],
                language=info.get('language'), format_note=info.get('format_note'),
                category_evidence=dict(upload_date=info.get('upload_date'), audio_languages=info.get('audio_languages')),
                process_attempts=1, metrics={'downloader_http_requests':None,'verified_replayed_audio_bytes':None,'media_wire_bytes':None})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--yt-dlp', type=pathlib.Path, required=True)
    parser.add_argument('--yt-dlp-python', type=pathlib.Path, help='Python interpreter for a non-executable yt-dlp zipapp')
    parser.add_argument('--ffmpeg', type=pathlib.Path, required=True)
    parser.add_argument('--node', type=pathlib.Path)
    parser.add_argument('--plugin-dir', type=pathlib.Path, help='Explicit isolated bgutil plugin checkout; defaults disabled')
    parser.add_argument('--po-provider-url', help='Existing loopback bgutil HTTP provider')
    parser.add_argument('--reference-client', choices=['default', 'mweb'], default='default')
    parser.add_argument('--native-client', choices=['auto', 'mweb'], default='auto')
    parser.add_argument('--video', action='append', required=True)
    parser.add_argument('--repetitions', type=int, default=1)
    parser.add_argument('--seed', type=int, default=0)
    parser.add_argument('--timeout', type=int, default=600)
    parser.add_argument('--native-resume-after-bytes', type=int)
    args = parser.parse_args()
    if args.repetitions < 1 or args.timeout < 1 or (args.native_resume_after_bytes is not None and args.native_resume_after_bytes < 1):
        parser.error('counts, limits and timeouts must be positive')
    if any(not re.fullmatch(r'[A-Za-z0-9_-]{11}', video) for video in args.video):
        parser.error('--video takes an exact 11-character ID')
    for name in ['yt_dlp', 'yt_dlp_python', 'ffmpeg', 'node']:
        path = getattr(args, name)
        if path:
            setattr(args, name, path.resolve(strict=True))
    if args.plugin_dir:
        args.plugin_dir = args.plugin_dir.resolve(strict=True)
    if args.po_provider_url:
        url = urlsplit(args.po_provider_url)
        if (not args.plugin_dir or url.scheme != 'http' or url.hostname not in ['127.0.0.1', '::1']
                or not url.port or url.username or url.password or url.path not in ['', '/'] or url.query or url.fragment):
            parser.error('--po-provider-url requires an explicit plugin and an HTTP loopback origin with a port')
    if not args.yt_dlp_python and not os.access(args.yt_dlp, os.X_OK):
        parser.error('yt-dlp is not executable; supply --yt-dlp-python for a Python zipapp')
    output = args.output.resolve()
    if not output.is_relative_to(REPO / 'runs'):
        parser.error('--output must be a new directory under this repository runs/')
    output.mkdir(parents=True, exist_ok=False, mode=0o700)
    report = dict(schema_version=1, support_scope='completed anonymous AAC/MP4 videos with finite length',
                  asr_tested=False, native_scenario='resume' if args.native_resume_after_bytes else 'fresh',
                  reference_scenario='fresh', seed=args.seed, trials=[], setup_complete=False,
                  configuration=dict(timeout_seconds=args.timeout, repetitions=args.repetitions,
                      native_resume_after_bytes=args.native_resume_after_bytes, reference_format='bestaudio[ext=m4a]',
                      reference_retries=0, reference_extractor_retries=0, reference_request_spacing_seconds=5,
                      reference_client=args.reference_client, native_client=args.native_client,
                      po_provider_enabled=bool(args.plugin_dir)),
                  limitations=['small local sample; no availability guarantee',
                               'different format variants and timing stages may prevent direct speed comparisons',
                               'reference excludes account cookies and default plugins; an explicit provider may be enabled; environment proxies may apply'])
    try:
        report['provenance'] = {
            'git_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=REPO, text=True).strip(),
            'tracked_patch_sha256': hashlib.sha256(subprocess.check_output(['git', 'diff', 'HEAD'], cwd=REPO)).hexdigest(),
            'cargo_lock_sha256': digest(REPO / 'Cargo.lock'),
            'harness_sha256': digest(pathlib.Path(__file__)),
            'proxy_environment_present': any(os.environ.get(name) for name in ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']),
            'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        }
        for name in ['yt_dlp', 'yt_dlp_python', 'ffmpeg', 'node']:
            path = getattr(args, name)
            if path:
                report['provenance'][name + '_sha256'] = digest(path)
        if args.plugin_dir:
            report['provenance']['plugin_revision'] = subprocess.check_output(
                ['git', '-C', str(args.plugin_dir), 'rev-parse', 'HEAD'], text=True).strip()
            report['provenance']['plugin_dirty'] = bool(subprocess.check_output(
                ['git', '-C', str(args.plugin_dir), 'status', '--porcelain'], text=True).strip())
        built, _ = run(['cargo', 'build', '--release', '--locked'], output, 'build', args.timeout)
        if built['exit_code'] or built['timeout']:
            raise ValueError('release build failed')
        built, stdout = run(['cargo', 'test', '--release', '--locked', '--lib', '--no-run', '--message-format=json'], output, 'probe-build', args.timeout)
        if built['exit_code'] or built['timeout']:
            raise ValueError('probe build failed')
        artifacts = [json.loads(line) for line in stdout.splitlines() if line.startswith('{')]
        executable = pathlib.Path(next(row['executable'] for row in artifacts if row.get('reason') == 'compiler-artifact'
                                      and row.get('target', {}).get('name') == 'agent-youtube' and row.get('profile', {}).get('test') and row.get('executable')))
        report['provenance']['native_sha256'] = digest(REPO / 'target/release/agent-youtube')
        report['provenance']['probe_sha256'] = digest(executable)
        report['setup_complete'] = True
        schedule = trial_schedule(args.video, args.repetitions, args.seed)
        cooldown = False
        for index, (tool, video, rep) in enumerate(schedule):
            folder = output / f'{index:03d}-{tool}-{video}-{rep}'
            folder.mkdir(mode=0o700)
            row = dict(tool=tool, video_id=video, repetition=rep, complete=False)
            try:
                if cooldown:
                    row['reason'] = 'skipped_server_cooldown'
                else:
                    row.update(native(args, executable, folder, video) if tool == 'agent-youtube' else reference(args, folder, video))
            except (OSError, ValueError, KeyError, TypeError, StopIteration, wave.Error) as error:
                # Do not serialize arbitrary exception text that might contain remote URLs.
                row['reason'] = 'tool_or_evidence_io_failed' if isinstance(error, OSError) else 'invalid_or_missing_evidence'
                row['exception_type'] = type(error).__name__
            # Missing/invalid result JSON must not discard a 429 seen in stderr.
            for log in folder.glob('*.stderr'):
                if re.search(r'HTTP(?: Error| error| status)?[ :]+429|Too Many Requests|youtube_rate_limited', log.read_text(), re.I):
                    row['rate_limited'] = True
            cooldown |= row.get('rate_limited', False)
            report['trials'].append(row)
            (folder / 'trial.json').write_text(json.dumps(row, indent=2))
            report['counts'] = summarize(report['trials'])
            (output / 'report.json').write_text(json.dumps(report, indent=2))
            print(json.dumps(dict(event='trial', tool=tool, video_id=video, complete=row['complete'], reason=row.get('reason'))), file=sys.stderr)
    except (OSError, ValueError, KeyError, TypeError, StopIteration, subprocess.SubprocessError) as error:
        report['setup_error'] = type(error).__name__
    report['counts'] = summarize(report['trials'])
    report['passed'] = report['setup_complete'] and bool(report['trials']) and all(row['complete'] for row in report['trials'])
    (output / 'report.json').write_text(json.dumps(report, indent=2))
    print(json.dumps(dict(report=str(output / 'report.json'), passed=report['passed'], counts=report['counts'])))
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
