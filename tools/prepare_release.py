#!/usr/bin/env python3
"""Prepare a complete local-only source/binary release, never upload it.

Unlike cargo package, git archive includes the required nested dependency forks.
Requires a clean committed checkout; private history/state never enter the bundle.
"""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tarfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PRIVATE = {'.git', 'backlog', '.backlog', 'backlog.config.yml', 'runs', 'target',
           'tools-local', 'models', '.grok', '.serena', '.claude', '.codex', '.cursor',
           '.kimi', '.kimi-code', '.agents', '.mcp.json', '.omc', '.omx', '.superpowers',
           '.gstack', '.claude-memory-compiler'}


def allowed_path(name):
    path = PurePosixPath(name)
    return bool(name) and not path.is_absolute() and '..' not in path.parts and all(
        part not in PRIVATE and not part.startswith(('.env', '.aider', '.codebase-memory'))
        for part in path.parts)


def compress_source(raw, prefix):
    with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as archive:
        for entry in archive:
            if entry.name == prefix.rstrip('/') and entry.isdir():
                continue
            if not entry.name.startswith(prefix):
                raise ValueError('archive entry outside source root')
            name = entry.name.removeprefix(prefix)
            if not allowed_path(name) or not (entry.isfile() or entry.isdir()):
                raise ValueError('private, linked or unsafe source archive entry')
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode='wb', filename='', mtime=0) as compressed:
        compressed.write(raw)
    return out.getvalue()


def validate_identity(producer, version, revision):
    if (producer.get('version') != version or producer.get('revision') != revision
            or producer.get('source_dirty') is not False):
        raise ValueError('binary does not identify the exact clean release commit')


def sha(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def git(*args):
    return subprocess.check_output(['git', *args], cwd=ROOT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--binary', type=Path, help='optional freshly built committed CLI')
    args = parser.parse_args()
    if git('status', '--porcelain', '--untracked-files=normal').strip():
        parser.error('commit intended changes before preparing an immutable release')
    revision = git('rev-parse', 'HEAD').decode().strip()
    package = tomllib.loads(git('show', 'HEAD:Cargo.toml').decode())['package']
    if package.get('publish') is not False:
        parser.error('crate publish=false policy required')
    version = package['version']
    prefix = f'agent-youtube-{version}/'
    raw = git('archive', '--format=tar', '--prefix=' + prefix, revision)
    compressed = compress_source(raw, prefix)
    with tarfile.open(fileobj=io.BytesIO(raw), mode='r:') as archive:
        names = {e.name.removeprefix(prefix) for e in archive}
    for required in ['Cargo.toml', 'Cargo.lock', 'LICENSE', 'NOTICE',
                     'vendor/syom/Cargo.toml', 'vendor/bandlimit/Cargo.toml',
                     'vendor/rustls-rustcrypto-current/Cargo.toml',
                     'experiments/botguard/vendor/boa_engine/Cargo.toml',
                     'experiments/botguard/vendor/boa_ast/Cargo.toml']:
        if required not in names:
            parser.error('source bundle is missing a required fork/license')
    producer = None
    if args.binary:
        response = json.loads(subprocess.check_output(
            [str(args.binary.resolve()), '--version'], timeout=30))
        if response.get('ok') is not True or response.get('command') != 'version':
            parser.error('invalid CLI version response')
        producer = response['result']
        validate_identity(producer, version, revision)
        if producer.get('binary_sha256') != sha(args.binary):
            parser.error('binary changed during identity verification')
    output = args.output.absolute()
    if not output.is_relative_to(ROOT / 'runs'):
        parser.error('select an owned ignored output under runs/')
    # Resolve ancestors to prevent an ignored symlink from redirecting artifacts.
    if output.resolve() != output or output.exists():
        parser.error('output must be a new real owned directory')
    output.mkdir(mode=0o700, parents=True)
    source = output / f'agent-youtube-{version}-source.tar.gz'
    source.write_bytes(compressed)
    source.chmod(0o600)
    subprocess.run(['python3', str(ROOT/'tools/prepare_notices.py'),
                    '--output', str(output/'notices')], check=True, cwd=ROOT,
                   stdout=subprocess.DEVNULL)
    shutil.copyfile(ROOT/'LICENSE', output/'LICENSE')
    if args.binary:
        destination = output/'agent-youtube'
        shutil.copyfile(args.binary, destination)
        destination.chmod(0o700)
        if sha(destination) != producer['binary_sha256']:
            raise ValueError('binary changed while copying; incomplete release preserved')
    files = {p.relative_to(output).as_posix(): sha(p) for p in sorted(output.rglob('*')) if p.is_file()}
    manifest = {'schema_version': 1, 'version': version, 'revision': revision,
                'private': True, 'source_archive': source.name, 'producer': producer,
                'files_sha256': files, 'security_approval': 'not_granted',
                'runtime_models_bundled': False, 'uploaded': False}
    (output/'release.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (output/'release.json').chmod(0o600)
    print(json.dumps({'ok': True, 'version': version, 'revision': revision,
                      'manifest': str(output/'release.json'), 'uploaded': False}))
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
