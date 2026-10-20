"""P4 check: does the task set rerun stably, can its grades be trusted, and
how much does each task's cost vary between identical Runs?

Reads the baseline Runs under .local/harness-hub-p4/baseline/runs through
the data hub (`anchor-hub health --json`) for quality and cost per task, and
the runner's meta.json for wall time. Writes summary.json beside the
P4 report. Run `python3 run.py --regrade <runs>` first so that every Run is
graded by the current task definitions.
"""

import glob
import json
import math
from pathlib import Path
import statistics
import subprocess
import sys

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
RUNS = REPO / '.local/harness-hub-p4/baseline/runs'
HUB = REPO / 'rust/target/debug/anchor-hub'
OUT = HERE.parent / 'p4'
sys.path.insert(0, str(HERE))
import tasks  # noqa: E402

# Two-sided alpha 0.05, power 0.8: n per arm = 2 (1.96 + 0.84)^2 (sd / delta)^2.
Z = 2 * (1.96 + 0.84) ** 2


def per_arm(summary, change):
    """Runs per arm to detect a relative `change` of the mean."""
    if not summary or summary['n'] < 2 or summary['mean'] == 0:
        return None
    cv = summary['sd'] / summary['mean']
    return math.ceil(Z * (cv / change) ** 2)


def main():
    metas = [json.loads(Path(p).read_text()) for p in sorted(glob.glob(str(RUNS / '*/meta.json')))]
    # Runs stopped by the runner's watchdog have no result; they are counted
    # apart and not read for quality.
    finished = [m for m in metas if not m['guard']]
    hub = json.loads(subprocess.run([str(HUB), 'health', '--json']
                                    + [m['root'] for m in finished], check=True,
                                    capture_output=True, text=True).stdout)
    quality = hub['summary'].get('quality', {})
    walls, guards = {}, {}
    for meta in metas:
        if meta['guard']:
            guards[meta['task']] = guards.get(meta['task'], 0) + 1
        else:
            walls.setdefault(meta['task'], []).append(meta['wall_seconds'])
    rows = {}
    for task in tasks.TASKS.values():
        q = quality.get(f'{task.id} v{task.version}')
        if not q:
            continue
        wall = walls.get(task.id, [])
        rows[task.id] = {
            'tier': task.tier,
            'version': task.version,
            'runs': q['runs'],
            'stopped_by_watchdog': guards.get(task.id, 0),
            'mean_score': q['mean_score'],
            'pass_rate': q['pass_rate'],
            'failing_checks': {k: v for k, v in q['checks'].items() if v < 1},
            'wall_seconds': {'median': round(statistics.median(wall), 1),
                             'max': round(max(wall), 1)} if wall else None,
            'tokens': {'mean': round(q['tokens']['mean']), 'sd': round(q['tokens']['sd']),
                       'per_arm_20pct': per_arm(q['tokens'], 0.2),
                       'per_arm_10pct': per_arm(q['tokens'], 0.1)},
            'agent_ms': {'mean': round(q['agent_ms']['mean']), 'sd': round(q['agent_ms']['sd']),
                         'per_arm_20pct': per_arm(q['agent_ms'], 0.2),
                         'per_arm_10pct': per_arm(q['agent_ms'], 0.1)},
            'tool_calls_median': q['tool_calls']['median'],
        }
    summary = {
        'runs': len(metas),
        'graded': sum(r['runs'] for r in rows.values()),
        'stopped_by_watchdog': sum(guards.values()),
        'tasks': rows,
        'hub_findings': sorted({(f['code'], f['subject']) for f in hub['findings']}),
        'tools': {tool: {k: v[k] for k in ('calls', 'success_rate', 'failures')}
                  for tool, v in hub['summary'].get('tools', {}).items()},
    }
    OUT.mkdir(exist_ok=True)
    (OUT / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
