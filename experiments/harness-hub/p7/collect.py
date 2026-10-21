"""Collect the finished blind trial's observed cost, calls and immutable outputs."""

import argparse
from collections import Counter
import json
from pathlib import Path
import shutil
import sys
import tarfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE.parent / 'p0'))
import extract  # noqa: E402
import run_p0  # noqa: E402


def collect(root, output):
    assert (root / 'meta.json').exists(), 'wait for the Host and configuration cleanup'
    assert not (root / 'state/managed/model-config.json').exists()
    # Check the uncompressed source before making a distributable archive.
    # Report counts only, without printing credential values or matched text.
    values = extract.credentials(REPO)
    leaked = [path for path in root.rglob('*')
              if path.is_file() and any(value in path.read_bytes() for value in values)]
    assert not leaked, 'credential bytes found; do not distribute the collected output'
    output.mkdir(parents=True, exist_ok=False)
    metrics = extract.extract(root)
    tools = Counter()
    per_node = []
    calls = []
    for path in sorted((root / 'state/goose-acp').glob('*.evidence.json')):
        evidence = json.loads(path.read_text())
        key = evidence['key']
        counts = Counter(call['tool'] for call in evidence.get('tool_calls', []))
        tools.update(counts)
        per_node.append({'key': key, 'model': evidence.get('model'),
                         'usage': evidence.get('usage'), 'tools': dict(counts),
                         'observed_model_requests': evidence.get('observed_model_requests'),
                         'real_model_calls': evidence.get('real_model_calls'),
                         'tool_calls_dropped': evidence.get('tool_calls_dropped')})
        calls.extend({'key': key, **call} for call in evidence.get('tool_calls', []))
    metrics['records_calls'] = sum(n for tool, n in tools.items() if tool.endswith('_records'))
    metrics['per_node'] = per_node
    metrics['comparability'] = 'No same-input old-RSI cost baseline was run.'
    run_p0.save(output / 'metrics.json', metrics)
    (output / 'tool-calls.jsonl').write_text(''.join(json.dumps(call, ensure_ascii=False) + '\n'
                                                 for call in calls))
    for path in sorted((root / 'state/artifacts').glob('*/manifest.json')):
        manifest = json.loads(path.read_text())
        key = manifest['key']
        destination = output / 'artifacts' / key['node_id'] / str(key['invocation'])
        shutil.copytree(path.parent / 'files', destination)
        shutil.copyfile(path, destination / 'artifact-manifest.json')
    # Preserve complete native sessions, ACP events, tool observations and
    # Kernel history. Exclude managed model settings and transient sockets.
    with tarfile.open(output / 'raw-trace.tar.gz', 'w:gz') as archive:
        for name in ('state/goose-acp', 'state/kernel-runs', 'state/kernel-facts',
                     'state/facts', 'state/artifacts', 'work/.goose-process',
                     'identity.json', 'meta.json', 'response.json', 'source-manifest.json',
                     'started.json', 'process.json',
                     'mcp-preflight.json', 'stderr.log', 'stdout.bin', 'bundle', 'operator'):
            path = root / name
            if path.exists():
                archive.add(path, arcname=name)
    metrics['files_with_credentials'] = len(leaked)
    run_p0.save(output / 'metrics.json', metrics)
    run_p0.save(output / 'collection.json', {
        'root': str(root), 'files_with_credentials': len(leaked),
        'raw_trace_sha256': run_p0.sha(output / 'raw-trace.tar.gz'),
        'meta': json.loads((root / 'meta.json').read_text()),
    })
    assert not leaked, 'credential bytes found; do not distribute the collected output'
    print(json.dumps({key: metrics[key] for key in ('phase', 'outcome', 'total_tokens',
                                                   'records_calls', 'files_with_credentials')}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('root', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    collect(args.root.resolve(), args.output.resolve())
