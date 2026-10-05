#!/usr/bin/env python3
"""Seed four admitted GGUF inputs in the owned workflow release cache."""
from contextlib import contextmanager
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import signal
import subprocess
import sys
import tempfile

sys.dont_write_bytecode = True
MODELS = {'model-qwen3.5-0.8b', 'model-qwen3.5-4b', 'model-qwen3.5-9b', 'model-bonsai-8b-q1'}


def need(condition, message):
    if not condition:
        raise ValueError(message)


@contextmanager
def curl_stream(source):
    command = ['/usr/bin/curl', '--disable', '--fail', '--silent', '--show-error', '--location',
               '--proto', '=https', '--proto-redir', '=https', '--max-redirs', '5',
               '--connect-timeout', '30', '--max-time', '900', '--max-filesize', str(source['max_bytes']),
               '--url', source['url']]
    child = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        yield child.stdout
        need(child.wait(timeout=5) == 0, 'Pinned model transfer failed')
    finally:
        child.stdout.close()
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)


def seed(recipes, cache, upstream, growth, fetch=curl_stream):
    selected = [recipe for recipe in recipes['recipes'] if recipe['component'] in MODELS]
    need(len(selected) == 4 and {recipe['component'] for recipe in selected} == MODELS,
         'The four admitted model recipes are required')
    for recipe in selected:
        need(recipe['platform'] == '*' and recipe['format'] == 'raw', 'Model recipe format differs')
        source = upstream.source_spec(recipe['source'], 12 * 1024**3)
        need(source.get('url') and source['checksum'].startswith('sha256:'), 'Model URL and SHA-256 pin required')
    need(sum(recipe['source']['max_bytes'] for recipe in selected) <= growth, 'Model bounds exceed planned growth')
    cache = upstream.directory(cache)
    consumed = 0
    upstream.disk_gate(cache, growth)
    for recipe in selected:
        source = recipe['source']
        destination = cache / ('sha256-' + source['checksum'].split(':')[1])
        if destination.exists() or destination.is_symlink():
            upstream.cached_input(source, cache)
            consumed += destination.stat().st_size
            continue
        upstream.disk_gate(cache, growth - consumed)
        descriptor, temporary = tempfile.mkstemp(prefix='.model-', dir=cache)
        temporary = Path(temporary)
        try:
            digest, size = hashlib.sha256(), 0
            with os.fdopen(descriptor, 'wb') as output, fetch(source) as stream:
                while block := stream.read(1024**2):
                    need(size + len(block) <= source['max_bytes'], 'Model transfer exceeds recorded size bound')
                    # Cache bytes consume the existing growth budget. Check the
                    # actual write and its remaining share before every block.
                    upstream.disk_gate(cache, max(len(block), growth - consumed - size))
                    output.write(block)
                    digest.update(block)
                    size += len(block)
                output.flush()
                os.fsync(output.fileno())
            need(size > 0 and digest.hexdigest() == source['checksum'].split(':')[1], 'Model checksum differs')
            os.link(temporary, destination)
            temporary.unlink()
            upstream.cached_input(source, cache)
            consumed += size
        finally:
            temporary.unlink(missing_ok=True)


def git(root, *arguments):
    return subprocess.check_output(['git', '-C', str(root), *arguments], text=True).strip()


def main():
    need(len(sys.argv) == 1 and os.environ.get('GITHUB_ACTIONS') == 'true'
         and os.environ.get('CI') == 'true' and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted'
         and sys.platform == 'darwin' and pwd.getpwuid(os.geteuid()).pw_name == 'runner',
         'Model preseed requires the disposable hosted Mac workflow')
    source, workflow = Path.cwd().resolve(), Path(__file__).resolve().parents[2]
    root, temp = Path(os.environ['OWNED_ROOT']), Path(os.environ['RUNNER_TEMP'])
    need(source.is_relative_to('/Users/runner/work') and workflow.is_relative_to('/Users/runner/work')
         and temp.is_absolute() and temp.is_dir() and temp.resolve() == temp
         and temp.is_relative_to('/Users/runner/work') and root.parent == temp
         and root.name == 'union-mac-' + os.environ['GITHUB_RUN_ID'] + '-' + os.environ['GITHUB_RUN_ATTEMPT']
         and root.is_dir() and root.resolve() == root and root.stat().st_uid == os.geteuid()
         and root.stat().st_mode & 0o077 == 0, 'Owned model cache ancestry differs')

    def source_binding():
        need(git(source, 'rev-parse', 'HEAD') == os.environ['SOURCE_COMMIT']
             and git(source, 'rev-parse', 'HEAD^{tree}') == os.environ['SOURCE_TREE']
             and not git(source, 'status', '--porcelain=v1', '--untracked-files=all'), 'Admitted model source differs')

    source_binding()
    need(git(workflow, 'rev-parse', 'HEAD') == os.environ['GITHUB_SHA']
         and not git(workflow, 'status', '--porcelain=v1', '--untracked-files=all'), 'Pinned workflow helper differs')
    need(os.environ['PREPARE_MODELS'] in ('true', 'false'), 'Model preparation mode differs')
    growth = (70 if os.environ['PREPARE_MODELS'] == 'true' else 50) * 1024**3
    cache = root / 'upstream-cache'
    need(not cache.exists() and not cache.is_symlink(), 'Model preseed cache must be fresh')
    spec = importlib.util.spec_from_file_location('upstream', source / 'scripts/release-upstream-input.py')
    upstream = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(upstream)
    seed(json.loads((source / 'scripts/release-upstream-recipes.json').read_bytes()), cache, upstream, growth)
    source_binding()
    with open(os.environ['GITHUB_ENV'], 'a') as environment:
        environment.write('ELASTOS_RELEASE_UPSTREAM_CACHE=' + str(cache) + '\n')


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt('terminated')))
    main()
