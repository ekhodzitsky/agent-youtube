#!/usr/bin/env python3
"""Verify candidate TLS versions against owned loopback OpenSSL test oracles.

Private fixture keys and logs stay in the named ignored run. OpenSSL is only a
research oracle, never a production dependency or trust-validation bypass.
"""
import argparse
import os
import hashlib
import json
from pathlib import Path
import socket
import subprocess
import time
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def sha(path):
    with path.open('rb') as file:
        return hashlib.file_digest(file, 'sha256').hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--application-probe', type=Path, help='test-only executable using the production factory; requires both versions')
    parser.add_argument('--group', choices=['X25519', 'P-256', 'P-384'], help='force the test server key-exchange group')
    parser.add_argument('--certificate', choices=['ecdsa','rsa'], default='ecdsa')
    parser.add_argument('--record-limits', action='store_true', help='exercise reduced AES-GCM key rotation/closure with the application probe')
    args = parser.parse_args()
    if args.record_limits and not args.application_probe:
        parser.error('--record-limits requires --application-probe')
    run = args.run.resolve()
    if not run.is_relative_to(ROOT / 'runs'):
        parser.error('owned ignored qualification run required')
    if args.application_probe:
        binary = args.application_probe.resolve()
        if not binary.is_relative_to(ROOT / 'runs'):
            parser.error('owned copied application probe required')
        report = {'provider': 'application'}
        protocols = {'tls12': True, 'tls13': True}
    else:
        report = json.loads((run / 'report.json').read_text())
        protocol_lines = [json.loads(line.removeprefix('TLS_CAPABILITIES ')) for line in (run / 'protocols.log').read_text().splitlines() if line.startswith('TLS_CAPABILITIES ')]
        if len(protocol_lines) != 1:
            raise ValueError('completed protocol capability probe required')
        protocols = protocol_lines[0]
        binary = run / 'target/release/examples/tls_prototype_capabilities'
    before = sha(binary)
    folder = Path(tempfile.mkdtemp(prefix='protocol-fixture-', dir=run))
    config = folder / 'leaf.cnf'
    config.write_text('[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:fixture.invalid\n')
    leaf_key = ['ec', '-pkeyopt', 'ec_paramgen_curve:P-256'] if args.certificate == 'ecdsa' else ['rsa:2048']
    commands = [
        ['openssl', 'req', '-x509', '-newkey', 'ec', '-pkeyopt', 'ec_paramgen_curve:P-256', '-nodes', '-days', '1', '-subj', '/CN=Owned TLS test root', '-keyout', str(folder / 'ca.key'), '-out', str(folder / 'ca.pem')],
        ['openssl', 'req', '-new', '-newkey', *leaf_key, '-nodes', '-subj', '/CN=fixture.invalid', '-keyout', str(folder / 'leaf.key'), '-out', str(folder / 'leaf.csr')],
        ['openssl', 'x509', '-req', '-in', str(folder / 'leaf.csr'), '-CA', str(folder / 'ca.pem'), '-CAkey', str(folder / 'ca.key'), '-CAcreateserial', '-days', '1', '-extfile', str(config), '-extensions', 'v3', '-out', str(folder / 'leaf.pem')],
        ['openssl', 'x509', '-in', str(folder / 'ca.pem'), '-outform', 'DER', '-out', str(folder / 'ca.der')],
    ]
    with (folder / 'generate.log').open('w') as log:
        for command in commands:
            subprocess.run(command, stdout=log, stderr=log, check=True, timeout=15)
    results = []
    for version, flag in [('tls12', '-tls1_2'), ('tls13', '-tls1_3')]:
        with socket.socket() as reserved:
            reserved.bind(('127.0.0.1', 0))
            port = reserved.getsockname()[1]
        with (folder / (version + '-server.log')).open('w') as log:
            server = subprocess.Popen(['openssl', 's_server', '-accept', f'127.0.0.1:{port}',
                '-cert', str(folder / 'leaf.pem'), '-key', str(folder / 'leaf.key'), flag, '-rev' if args.record_limits else '-www', *(['-msg', '-ciphersuites', 'TLS_AES_128_GCM_SHA256', '-cipher', 'ECDHE-RSA-AES128-GCM-SHA256' if args.certificate == 'rsa' else 'ECDHE-ECDSA-AES128-GCM-SHA256'] if args.record_limits else []), *(['-groups', args.group] if args.group else [])], stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if server.poll() is not None:
                        raise RuntimeError('local TLS oracle stopped')
                    try:
                        with socket.create_connection(('127.0.0.1', port), timeout=.1):
                            break
                    except OSError:
                        time.sleep(.05)
                else:
                    raise RuntimeError('local TLS oracle did not listen')
                with (folder / (version + '-client.log')).open('w') as output:
                    command = [str(binary), f'127.0.0.1:{port}', str(folder / 'ca.der')]
                    env = dict(os.environ)
                    if args.application_probe:
                        command = [str(binary), '--ignored', '--exact', 'download::tls::tests::research_tls_protocol_handshake', '--nocapture']
                        env.update(AGENT_YOUTUBE_TLS_PROBE_ADDRESS=f'127.0.0.1:{port}', AGENT_YOUTUBE_TLS_PROBE_CA=str(folder / 'ca.der'))
                    if args.record_limits:
                        env['AGENT_YOUTUBE_TLS_PROBE_RECORD_LIMIT'] = '1'
                    client = subprocess.run(command, env=env, stdout=output, stderr=output, timeout=10)
                lines = (folder / (version + '-client.log')).read_text().splitlines()
                actual = [json.loads(line.removeprefix('TLS_HANDSHAKE ')) for line in lines if line.startswith('TLS_HANDSHAKE ')]
                success = client.returncode == 0
                expected = protocols[version]
                if success and (len(actual) != 1 or not actual[0]['protocol'].endswith(('TLSv1_2)' if version == 'tls12' else 'TLSv1_3)'))):
                    raise RuntimeError('successful probe did not prove the negotiated protocol')
                if not success and not any('InvalidData' in line or 'HandshakeFailure' in line or 'PeerIncompatible' in line for line in lines):
                    raise RuntimeError('negative probe did not reach a TLS handshake failure')
                record_evidence = {}
                if args.record_limits:
                    limits = [json.loads(line.removeprefix('TLS_RECORD_LIMIT ')) for line in lines if line.startswith('TLS_RECORD_LIMIT ')]
                    server_log = (folder / (version + '-server.log')).read_text()
                    if not success or len(limits) != 1:
                        raise RuntimeError('record-limit probe did not complete')
                    if version == 'tls13' and 'KeyUpdate' not in server_log:
                        raise RuntimeError('oracle did not authenticate a TLS1.3 KeyUpdate')
                    if version == 'tls12' and 'close_notify' not in server_log:
                        raise RuntimeError('oracle did not receive the TLS1.2 close alert')
                    record_evidence = dict(record_limits=limits[0], oracle_key_update_messages=server_log.count('KeyUpdate'))
                group = actual[0].get('group') if success else None
                expected_group = {'X25519':'Some(X25519)', 'P-256':'Some(secp256r1)', 'P-384':'Some(secp384r1)'}.get(args.group)
                if args.application_probe and success and expected_group and group != expected_group:
                    raise RuntimeError('handshake did not prove the forced group')
                results.append(dict(version=version, succeeded=success, expected_success=expected,
                                    matches_expected=success == expected, exit_code=client.returncode,
                                    negotiated=actual[0]['protocol'] if success else None, group=group, **record_evidence))
            finally:
                server.terminate()
                try:
                    server.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    server.kill()
                    server.wait(timeout=5)
    if sha(binary) != before:
        raise RuntimeError('protocol probe executable changed during checks')
    result = dict(schema_version=1, provider=report['provider'], certificate=args.certificate, record_limit_probe=args.record_limits, probe_sha256=before,
                  driver_sha256=sha(Path(__file__)), fixture_ca_sha256=sha(folder / 'ca.der'),
                  results=results, complete=all(r['matches_expected'] for r in results))
    (run / ('handshakes' + ('-record-limits' if args.record_limits else '') + ('-rsa' if args.certificate == 'rsa' else '') + ('-' + args.group if args.group else '') + '.json')).write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))
    return 0 if result['complete'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
