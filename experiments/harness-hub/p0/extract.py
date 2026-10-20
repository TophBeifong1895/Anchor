"""Extract one metrics row per P0 Run from its isolated root. No model text is kept."""

import glob
import json
from pathlib import Path
import re
import sys

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
from fixtures import t1_expected  # noqa: E402

# Facts the current Host does not record; P2 must decide whether to add them.
UNAVAILABLE = [
    'kernel record time: Stamp has no timestamp, so per-submission time is unknown',
    'per tool call duration: tool_call has only second-resolution created, tool_call_update has no time',
    'op node duration: no per-node start/end fact for Op nodes',
    'agent node start/end wall time: only model-request elapsed_ms is observed',
    'configuration identity per submission: knobs is the constant "default"',
    'causal link between model turn, tool call and kernel submission',
]


def _load(path):
    return json.loads(Path(path).read_text())


def _instant(text):
    """RFC 3339 with up to nanosecond fractions; Python parses at most micro."""
    from datetime import datetime
    text = re.sub(r'(\.\d{6})\d+', r'\1', text.replace('Z', '+00:00'))
    return datetime.fromisoformat(text)


def _seconds(start, end):
    try:
        return (_instant(end) - _instant(start)).total_seconds()
    except (TypeError, ValueError):
        return None


def extract(root, secrets=()):
    root = Path(root)
    meta = _load(root / 'meta.json')
    response = _load(root / 'response.json') or {}
    projection = response.get('projection') or {}
    state = root / 'state'
    row = {k: meta[k] for k in ('task', 'index', 'run', 'started_unix', 'wall_seconds', 'guard', 'returncode')}
    row.update({'kind': response.get('kind'), 'phase': projection.get('phase'),
                'outcome': projection.get('outcome'), 'reason': projection.get('reason')})

    metadata = next(iter(glob.glob(str(state / 'run-metadata/*.json'))), None)
    observed = next(iter(glob.glob(str(state / 'kernel-run-observations/*.json'))), None)
    row['created_to_updated_seconds'] = (
        _seconds(_load(metadata)['created'], _load(observed)['updated']) if metadata and observed else None)

    usage = {k: 0 for k in ('messages', 'compaction_messages', 'input_tokens', 'output_tokens',
                             'total_tokens', 'cache_read_tokens', 'cache_write_tokens', 'elapsed_ms')}
    cost, ttft, tools, tool_failures, dropped, invocations, models = 0.0, [], {}, {}, 0, 0, set()
    for path in glob.glob(str(state / 'goose-acp/*.evidence.json')):
        evidence = _load(path)
        invocations += 1
        models.add(evidence.get('model'))
        metrics = evidence.get('usage') or {}
        for key in usage:
            usage[key] += metrics.get(key) or 0
        cost += metrics.get('cost_usd') or 0
        ttft += [s['time_to_first_token_ms'] for s in metrics.get('samples', [])
                 if s.get('time_to_first_token_ms') is not None]
        for call in evidence.get('tool_calls') or []:
            name = call.get('tool')
            tools[name] = tools.get(name, 0) + 1
            if not (call.get('result') or {}).get('ok', False):
                tool_failures[name] = tool_failures.get(name, 0) + 1
        dropped += evidence.get('tool_calls_dropped') or 0
    row.update(usage)
    row.update({'agent_invocations': invocations, 'models': sorted(m for m in models if m),
                'cost_usd': round(cost, 6), 'ttft_ms_mean': sum(ttft) / len(ttft) if ttft else None,
                'ttft_ms_max': max(ttft) if ttft else None,
                'tool_calls': sum(tools.values()), 'tool_calls_by_name': tools,
                'tool_failures': sum(tool_failures.values()), 'tool_failures_by_name': tool_failures,
                'tool_calls_dropped': dropped})

    records, reasons, effects, summary = 0, {}, {}, None
    for path in glob.glob(str(state / 'kernel-runs/*/history.jsonl')):
        for line in Path(path).read_text().splitlines():
            if not line.strip():
                continue
            record = json.loads(line)
            records += 1
            verdict = record['verdict']
            reasons[verdict.get('reason')] = reasons.get(verdict.get('reason'), 0) + 1
            effects[verdict.get('effect')] = effects.get(verdict.get('effect'), 0) + 1
            content = record['submission'].get('content')
            if isinstance(content, dict) and content.get('summary') is not None and summary is None:
                summary = content['summary']
    row.update({'kernel_records': records, 'kernel_reasons': reasons, 'kernel_effects': effects,
                'kernel_refused': effects.get('refused', 0), 'kernel_stale': reasons.get('stale_version', 0)})

    done = row['phase'] == 'settled' and row['outcome'] == 'done'
    if meta['task'] == 'T1':
        row['passed'] = done and (summary or '').strip().strip('`') == t1_expected()
    else:
        row['passed'] = done  # the verify Op fails the Run unless result.txt is exact
    row['status'] = 'guard' if meta['guard'] else ('passed' if row['passed'] else 'failed')

    leaked = 0
    for path in root.rglob('*'):
        if path.is_file() and secrets:
            try:
                data = path.read_bytes()
            except OSError:
                continue
            leaked += any(secret in data for secret in secrets)
    row['files_with_credentials'] = leaked
    return row


def credentials(repo):
    config = _load(repo / '.local/rust/state/managed/model-config.json')
    return tuple(v.encode() for v in (config.get('credentials') or {}).values() if v)


if __name__ == '__main__':
    repo = HERE.parents[2]
    secrets = credentials(repo)
    for root in sys.argv[1:]:
        print(json.dumps(extract(root, secrets), ensure_ascii=False))
