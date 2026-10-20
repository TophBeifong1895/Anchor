"""Summarize P0 Runs: dispersion, drift and minimum detectable differences.

Reads every Run root under .local/harness-hub-p0/runs (trial runs with index 0
are excluded), writes runs.jsonl and summary.json next to this script.
"""

import json
import math
from pathlib import Path
import statistics
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
sys.path.insert(0, str(HERE))
from extract import UNAVAILABLE, credentials, extract  # noqa: E402

METRICS = ['wall_seconds', 'created_to_updated_seconds', 'elapsed_ms', 'ttft_ms_mean', 'messages',
           'input_tokens', 'output_tokens', 'total_tokens', 'cache_read_tokens', 'cost_usd',
           'tool_calls', 'tool_failures', 'kernel_records',
           # Derived: path length explains most of the variance of totals.
           'tokens_per_message', 'wall_per_message']
ARMS = (10, 30, 100)
Z = 1.959964 + 0.841621  # two-sided alpha 0.05, power 0.8


def ranks(values):
    order = sorted(range(len(values)), key=lambda i: values[i])
    result = [0.0] * len(values)
    i = 0
    while i < len(order):
        j = i
        while j + 1 < len(order) and values[order[j + 1]] == values[order[i]]:
            j += 1
        for k in range(i, j + 1):
            result[order[k]] = (i + j) / 2 + 1
        i = j + 1
    return result


def spearman(x, y):
    if len(set(y)) < 2:
        return None
    rx, ry = ranks(x), ranks(y)
    mx, my = statistics.fmean(rx), statistics.fmean(ry)
    num = sum((a - mx) * (b - my) for a, b in zip(rx, ry))
    den = math.sqrt(sum((a - mx) ** 2 for a in rx) * sum((b - my) ** 2 for b in ry))
    return num / den if den else None


def quantile(values, q):
    ordered = sorted(values)
    position = (len(ordered) - 1) * q
    low, high = math.floor(position), math.ceil(position)
    return ordered[low] + (ordered[high] - ordered[low]) * (position - low)


def describe(rows, metric):
    values = [r[metric] for r in rows if r.get(metric) is not None]
    if len(values) < 2:
        return {'n': len(values)}
    mean, sd = statistics.fmean(values), statistics.stdev(values)
    result = {'n': len(values), 'mean': mean, 'sd': sd, 'cv': sd / mean if mean else None,
              'median': statistics.median(values), 'p10': quantile(values, .1),
              'p90': quantile(values, .9), 'min': min(values), 'max': max(values),
              'drift_spearman': spearman([r['index'] for r in rows if r.get(metric) is not None], values)}
    result['mde_relative'] = {str(n): (Z * sd * math.sqrt(2 / n) / mean if mean else None) for n in ARMS}
    return result


def wilson(successes, n, z=1.959964):
    if n == 0:
        return None
    p = successes / n
    centre = (p + z * z / (2 * n)) / (1 + z * z / n)
    half = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / (1 + z * z / n)
    return [centre - half, centre + half]


def pass_rate(rows):
    n = len(rows)
    passed = sum(r['passed'] for r in rows)
    interval = wilson(passed, n)
    # Variance at a 100% ceiling is unobservable; use the Wilson lower bound as a conservative p.
    p = min(passed / n, interval[0]) if passed == n else passed / n
    mde = {str(k): Z * math.sqrt(2 * p * (1 - p) / k) for k in ARMS}
    return {'n': n, 'passed': passed, 'rate': passed / n, 'wilson95': interval,
            'mde_absolute': mde, 'ceiling': passed == n}


def main():
    secrets = credentials(REPO)
    roots = sorted((REPO / '.local/harness-hub-p0/runs').iterdir())
    rows = [extract(root, secrets) for root in roots if (root / 'meta.json').exists()]
    rows = [r for r in rows if r['index'] > 0]
    for r in rows:
        r['tokens_per_message'] = r['total_tokens'] / r['messages'] if r['messages'] else None
        r['wall_per_message'] = r['wall_seconds'] / r['messages'] if r['messages'] else None
    with (HERE / 'runs.jsonl').open('w') as out:
        for row in sorted(rows, key=lambda r: (r['task'], r['index'])):
            out.write(json.dumps(row, ensure_ascii=False) + '\n')
    summary = {'unavailable': UNAVAILABLE, 'tasks': {}}
    for task in sorted({r['task'] for r in rows}):
        group = sorted((r for r in rows if r['task'] == task), key=lambda r: r['index'])
        statuses = {}
        for r in group:
            statuses[r['status']] = statuses.get(r['status'], 0) + 1
        summary['tasks'][task] = {
            'runs': len(group), 'statuses': statuses,
            'models': sorted({m for r in group for m in r['models']}),
            'files_with_credentials': sum(r['files_with_credentials'] for r in group),
            'pass_rate': pass_rate(group),
            'tool_calls_by_name': _merge(r['tool_calls_by_name'] for r in group),
            'tool_failures_by_name': _merge(r['tool_failures_by_name'] for r in group),
            'kernel_reasons': _merge(r['kernel_reasons'] for r in group),
            'metrics': {m: describe(group, m) for m in METRICS},
        }
    identity = {}
    for path in (REPO / '.local/harness-hub-p0').glob('identity-*.json'):
        identity[path.stem.split('-', 1)[1]] = json.loads(path.read_text())
    summary['identity'] = {task: {k: v for k, v in value.items() if k != 'live_pid'}
                           for task, value in identity.items()}
    (HERE / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({t: {'statuses': v['statuses'], 'pass': v['pass_rate']['rate']}
                      for t, v in summary['tasks'].items()}))


def _merge(dicts):
    total = {}
    for d in dicts:
        for k, v in d.items():
            total[k] = total.get(k, 0) + v
    return total


if __name__ == '__main__':
    main()
