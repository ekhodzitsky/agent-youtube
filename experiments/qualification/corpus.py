"""Pinned research corpus and truthful multi-date aggregation; no network I/O."""
import collections
import datetime
import json
import pathlib
import re
import statistics
import qualify

DEFAULT_MANIFEST = pathlib.Path(__file__).with_name('corpus.json')


def load_manifest(path):
    data = json.loads(path.read_text())
    if data.get('schema_version') != 1 or not 15 <= len(data.get('videos', [])) <= 20:
        raise ValueError('unsupported corpus manifest')
    seen = set()
    for row in data['videos']:
        if (not re.fullmatch(r'[A-Za-z0-9_-]{11}', row.get('id', '')) or row['id'] in seen
                or not row.get('provenance') or not isinstance(row.get('candidate_tags'), list)):
            raise ValueError('invalid or duplicate corpus item')
        seen.add(row['id'])
    return data


def aggregate(reports, expected_days=3):
    hashes = {r.get('corpus_sha256') for r in reports}
    if len(hashes) > 1 or None in hashes:
        raise ValueError('different or unbound corpora cannot be pooled')
    if len({json.dumps(r.get('cohort'), sort_keys=True) for r in reports}) > 1:
        raise ValueError('different frozen tool cohorts cannot be pooled')
    rows = [row for report in reports for row in report.get('trials', [])]
    attempted_dates = sorted({datetime.datetime.fromisoformat(r['started_at']).date().isoformat() for r in reports if r.get('finished') and any(not row.get('reason', '').startswith('skipped_') for row in r.get('trials', []))})
    dates = sorted({datetime.datetime.fromisoformat(r['started_at']).date().isoformat() for r in reports})
    metrics = {}
    for tool in sorted({r['tool'] for r in rows}):
        attempted = [r for r in rows if r['tool'] == tool and not (r.get('reason') or '').startswith('skipped_')]
        times = [r['total_wall_seconds'] for r in attempted if r.get('complete') and isinstance(r.get('total_wall_seconds'), (int, float))]
        metrics[tool] = dict(media_wire_bytes=None, media_wire_bytes_reason='not instrumented; never inferred from committed bytes',
            requests_by_endpoint=qualify.sum_request_categories([r.get('metrics', {}) for r in attempted]),
            coverage=qualify.METRIC_COVERAGE if tool == 'agent-youtube' else 'reference request counters are not instrumented',
            median_success_total_wall_seconds=statistics.median(times) if times else None,
            downloader_http_requests=sum(r["metrics"]["downloader_http_requests"] for r in attempted) if attempted and all(isinstance(r.get("metrics", {}).get("downloader_http_requests"), int) for r in attempted) else None,
            acquisition_process_attempts=sum(r["process_attempts"] for r in attempted) if attempted and all(isinstance(r.get("process_attempts"), int) for r in attempted) else None,
            verified_replayed_audio_bytes=sum(r['metrics']['verified_replayed_audio_bytes'] for r in attempted)
                if attempted and all(isinstance(r.get('metrics', {}).get('verified_replayed_audio_bytes'), int) for r in attempted) else None)
    reasons = collections.Counter(r.get('reason') or r.get('error_code') or 'unclassified' for r in rows if not r.get('complete'))
    return dict(schema_version=1, observed_dates=dates, calendar_complete=len(attempted_dates) >= expected_days, attempted_dates=attempted_dates,
        series_reports=len(reports), corpus_sha256=next(iter(hashes), None), counts=qualify.summarize(rows),
        failure_and_skip_causes=dict(reasons), metrics=metrics,
        limitations=['current network only', 'candidate categories require observed evidence',
                     'different binaries/configurations must be compared as separate cohorts',
                     'complete media is not ASR accuracy or unconditional availability'])
