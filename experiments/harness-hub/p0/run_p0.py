"""P0 noise measurement runner.

Runs fixture Graphs through an isolated Host that uses the same executable as
the live 8077 service and a private snapshot of its saved model settings. Each
Run gets a fresh state root under .local/harness-hub-p0/runs; the live service
data is never written. The snapshot is deleted as soon as the Host exits.

Usage: python3 run_p0.py TASK COUNT   (TASK is T1 or T2)
"""

import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import sys
import time

os.umask(0o077)
HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LOCAL = REPO / '.local/harness-hub-p0'
GUARD_SECONDS = 900
FORWARD = ('PATH', 'LANG', 'LC_ALL', 'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY',
           'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy', 'SSL_CERT_FILE', 'SSL_CERT_DIR')

sys.path.insert(0, str(HERE))
from fixtures import graphs  # noqa: E402


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def live_host():
    """The running `serve` process from a deployment, and its environment."""
    for entry in Path('/proc').iterdir():
        if not entry.name.isdigit():
            continue
        try:
            exe = Path(os.readlink(entry / 'exe'))
            cmdline = (entry / 'cmdline').read_bytes().split(b'\0')
        except OSError:
            continue
        if exe.name == 'anchor-runner-host' and '/.local/rust/deployments/' in str(exe) \
                and b'serve' in cmdline:
            environ = dict(item.decode().split('=', 1)
                           for item in (entry / 'environ').read_bytes().split(b'\0') if b'=' in item)
            return int(entry.name), exe, environ
    raise SystemExit('live deployed Host not found')


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def run_once(task, index, host, live_env):
    graph = graphs()[task]
    run = f'p0-{task.lower()}-{index:02d}-{time.time_ns()}'
    root = LOCAL / 'runs' / run
    for name in ('home', 'library', 'operator', 'catalog'):
        (root / name).mkdir(parents=True)
    env = {k: v for k, v in live_env.items() if k in FORWARD or k.startswith(('ANCHOR_GOOSE_', 'ANCHOR_MODEL_'))}
    env.update({
        'HOME': str(root / 'home'),
        'ANCHOR_RUNNER_STATE_ROOT': str(root / 'state'),
        'ANCHOR_RUNNER_WORKSPACE_ROOT': str(root / 'work'),
        'ANCHOR_RUNNER_BUNDLE_ROOT': str(root / 'bundle'),
        'ANCHOR_RUNNER_CATALOG_ROOT': str(root / 'catalog'),
        'ANCHOR_RUNNER_LIBRARY_ROOT': str(root / 'library'),
        'ANCHOR_RUNNER_LOCAL_INPUTS_ROOT': str(root / 'operator'),
        'ANCHOR_RUNNER_GRAPH_NAME': f'p0-{task.lower()}',
        'ANCHOR_RUNNER_ALLOWED_COMMANDS': 'sh,cat,printf,grep,test,true,ls,mkdir',
        'ANCHOR_GOOSE_RELAY_BINARY': str(host.parent / 'anchor-net-relay'),
    })
    model = root / 'state/managed/model-config.json'
    model.parent.mkdir(parents=True)
    shutil.copyfile(REPO / '.local/rust/state/managed/model-config.json', model)
    model.chmod(0o600)
    save(root / 'bundle/graph.json', graph)
    save(root / 'bundle/manifest.json', {'format': 1, 'graph': 'graph.json', 'plugins': []})
    request = json.dumps({'op': 'start_bundle', 'version': 1, 'request_id': run,
                          'run_id': run, 'input': {}}).encode()
    started = time.time()
    guard = False
    process = subprocess.Popen([str(host)], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, env=env, start_new_session=True)
    try:
        try:
            stdout, stderr = process.communicate(struct.pack('>I', len(request)) + request,
                                                 timeout=GUARD_SECONDS)
        except subprocess.TimeoutExpired:
            guard = True
            os.killpg(process.pid, signal.SIGTERM)
            stdout, stderr = process.communicate(timeout=30)
    finally:
        model.unlink(missing_ok=True)
    ended = time.time()
    (root / 'stderr.log').write_bytes(stderr)
    response = None
    if len(stdout) >= 4:
        size = struct.unpack('>I', stdout[:4])[0]
        response = json.loads(stdout[4:4 + size])
    save(root / 'response.json', response)
    meta = {'task': task, 'index': index, 'run': run, 'root': str(root),
            'started_unix': started, 'ended_unix': ended, 'wall_seconds': ended - started,
            'returncode': process.returncode, 'guard': guard}
    save(root / 'meta.json', meta)
    return meta


def main():
    task, count = sys.argv[1], int(sys.argv[2])
    start = int(sys.argv[3]) if len(sys.argv) > 3 else 1
    pid, host, live_env = live_host()
    identity = {'live_pid': pid, 'host': str(host), 'host_sha256': sha(host),
                'goose_sha256': sha(live_env['ANCHOR_GOOSE_BINARY']) if 'ANCHOR_GOOSE_BINARY' in live_env else None}
    save(LOCAL / f'identity-{task}.json', identity)
    for index in range(start, start + count):
        meta = run_once(task, index, host, live_env)
        print(json.dumps({k: meta[k] for k in ('run', 'wall_seconds', 'returncode', 'guard')}), flush=True)


if __name__ == '__main__':
    main()
